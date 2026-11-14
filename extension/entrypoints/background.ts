import { findEarlyBrowserTakeoverByUrl, validDownloadIdentity, type EarlyBrowserTakeover } from '../lib/takeover'
import { browser } from 'wxt/browser'
import { downloadSelectionKey, localDownloadIntentUrl } from '../lib/resources'
import { mediaPushRequestId } from '../lib/mediaPush'
import { isSniffCurrentPageCommand, openMediaPanelCommandDispatch, shouldAggregateTabMedia } from '../lib/sniffCommand'
import { NativeBridge, type NativePortLike } from '../lib/nativeBridge'
import { boundedConfidence, canonicalMediaUrl, capturedRequestIdentity, classifyDownload, classifyPlaybackSource, classifyResource, compactResources, isConcreteDownloadMime, isShortLivedMediaSignatureUsable, mergeResources, normalizeHost, pageIdentity, pageResourceKey, pruneExpiredResources, replayableRequestHeaders, resourceBelongsToFrame, resourceFingerprint, resourceId, resourceRequestIdentity, shouldTakeover, suggestedResourceFilename, usesShortLivedMediaSignature, type DownloadClickIntent, type MediaResource } from '../lib/resources'
import { RequestChainStore, replayablePostRequest, requestHeader, responseHeader, responseIdentity, type RequestChain, type DownloadLike } from '../lib/requestChain'
import { browserCleanupAction, canContinueTakeover, canResumeBrowserDownload, desktopAcceptedHandoff, localFileTaskReadiness, handoffTerminalStatus, type BrowserHandoffPayload } from '../lib/takeover'
import {
  PAUSED_FOLLOW_UP_RECHECK_MS,
  PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY,
  PausedHandoffFollowUpStore,
  type PausedHandoffFollowUp,
  stepPausedHandoffFollowUp,
} from '../lib/pausedHandoffFollowups'
import { HANDOFF_SUPPRESSION_STORAGE_KEY, isHandoffSuppressed, normalizeHandoffSuppressions } from '../lib/handoffSuppression'
import { createRecurringAlarm, filenameDeterminationEvent, requestHeaderExtraInfo } from '../lib/browserCapabilities'
import { inspectHlsResource } from '../lib/hlsInspection'
import { inspectDashResource } from '../lib/dashInspection'
import { isMediaResponseStatus } from '../lib/manifestSniff'
import { contentDispositionFilename } from '../lib/contentDisposition'
import { adaptiveInspectionKey, InspectionCache } from '../lib/inspectionCache'
import { cookieHeaderForUrl, cookieLookupUrl, cookieMatchesUrl, cookiePartitionKeyForSrcdoc, cookiePartitionSite, cookiePermissionAllows, isCookieHostPermissionError, isSrcdocCookieHostPermissionError, normalizeCookiePermissionHosts, replayCookieIdentity, type ReplayCookie, type SrcdocFrameContext } from '../lib/browserCookies'
import { detectBrowserFamily, stableBrowserClientId } from '../lib/browserClient'
import { isEarlyDirectDownloadResponse, type ObservedDownloadResource } from '../lib/directResponse'
import { clickIntentPollsForKind, earlyTakeoverRequiresClick } from '../lib/fileTakeover'
import { isDirectDownloadLink } from '../lib/directLink'
import { ClickIntentStore } from '../lib/clickIntentStore'
import { TakeoverSettingsSync } from '../lib/takeoverSettingsSync'
import { SessionListStore } from '../lib/sessionListStore'
import { BlobSourceStore, type BlobSourceRecord } from '../lib/blobSourceStore'
import { contextMenuCapabilities } from '../lib/contextMenuActions'
import { installSubtitleBackground } from '../lib/subtitleBackground'

const HOST = 'com.ciaooo55.hls_downloader'
const dynamicContextMenus = browser.contextMenus as typeof browser.contextMenus & {
  onShown?: { addListener: (listener: (info: { linkUrl?: string, srcUrl?: string, mediaType?: string }) => void) => void }
  refresh?: () => Promise<void> | void
}
const CLICK_INTENT_STORAGE_KEY = 'click-intents'
const clickIntentStore = new ClickIntentStore(browser.storage.session, CLICK_INTENT_STORAGE_KEY)
// Images, stylesheets, scripts and fonts can dominate busy pages but cannot
// become a replayable browser download or adaptive media request. Keeping
// them out of the request-chain store avoids copying headers twice for every
// passive asset while preserving navigations, fetch/XHR, media and legacy
// object requests used by real downloads.
const TRACKED_REQUEST_FILTER = {
  urls: ['<all_urls>'],
  types: ['main_frame', 'sub_frame', 'xmlhttprequest', 'media', 'other', 'object'],
} as any
const determinedDownloads = new Map<number, Browser.downloads.DownloadItem>()
const determinationWaiters = new Map<number, (item: Browser.downloads.DownloadItem) => void>()
const LOCAL_DOWNLOADS_KEY = 'local-browser-download-handoffs'
interface LocalDownloadHandoff {
  tabId?: number
  downloadId: number
  requestId: string
  sourceKey: string
  pageUrl: string
  importUrl?: string
  handoffId?: string
  intent?: DownloadClickIntent
  retryAt?: number
  sourceStartedAt?: string
  sourceFilename?: string
}
const localDownloadHandoffs = new Map<string, LocalDownloadHandoff>()
let localDownloadsHydration: Promise<void> | undefined
let localDownloadsPass = false
let localDownloadsTimer: ReturnType<typeof setTimeout> | undefined
let localDownloadsWrite: Promise<unknown> = Promise.resolve()

async function hydrateLocalDownloads(): Promise<void> {
  localDownloadsHydration ??= browser.storage.local.get(LOCAL_DOWNLOADS_KEY).then(data => {
    const stored = data[LOCAL_DOWNLOADS_KEY]
    for (const record of Array.isArray(stored) ? stored as LocalDownloadHandoff[] : []) {
      if (Number.isInteger(record.downloadId) && typeof record.requestId === 'string') {
        if (record.retryAt) record.retryAt = Math.min(record.retryAt, Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS)
        localDownloadHandoffs.set(record.requestId, record)
      }
    }
  }).catch(error => {
    localDownloadsHydration = undefined
    throw error
  })
  await localDownloadsHydration
}

async function persistLocalDownloads(): Promise<void> {
  const records = [...localDownloadHandoffs.values()]
  localDownloadsWrite = localDownloadsWrite.catch(() => undefined)
    .then(() => browser.storage.local.set({ [LOCAL_DOWNLOADS_KEY]: records }))
  await localDownloadsWrite
}

async function queueLocalBrowserDownload(record: LocalDownloadHandoff): Promise<void> {
  await hydrateLocalDownloads()
  const [item] = await browser.downloads.search({ id: record.downloadId })
  record.sourceStartedAt = item?.startTime
  // 浏览器可在接收响应时按 MIME 调整文件名；完成后才固定导入路径。
  record.sourceFilename = item?.state === 'complete' ? item.filename : undefined
  const existing = [...localDownloadHandoffs.values()].find(candidate => candidate.downloadId === record.downloadId
      && (!candidate.sourceStartedAt || !record.sourceStartedAt || candidate.sourceStartedAt === record.sourceStartedAt)
    || candidate.sourceStartedAt && candidate.sourceStartedAt === record.sourceStartedAt
      && candidate.sourceFilename === record.sourceFilename && candidate.sourceKey === record.sourceKey)
  // 重复的浏览器事件必须继续同一份导入，不能丢失已经提交的交接回执。
  if (existing && existing.sourceKey !== record.sourceKey) {
    throw new Error('浏览器下载身份已变化，保留原文件并停止自动导入')
  }
  if (!existing) localDownloadHandoffs.set(record.requestId, record)
  await persistLocalDownloads()
  void continueLocalDownloadHandoffs().catch(error => console.warn('HLS Downloader local file handoff failed', error))
}

async function findLocalBrowserDownload(record: LocalDownloadHandoff): Promise<Browser.downloads.DownloadItem | undefined> {
  const [item] = await browser.downloads.search({ id: record.downloadId })
  const matches = async (candidate: Browser.downloads.DownloadItem) =>
    (!record.sourceStartedAt || candidate.startTime === record.sourceStartedAt)
    && (!record.sourceFilename || candidate.filename === record.sourceFilename)
    && await localDownloadIntentUrl(candidate.url) === record.sourceKey
  if (item && await matches(item)) return item
  // Firefox 的数字下载 ID 不跨浏览器会话持久；仅按稳定属性唯一匹配后重新绑定。
  if (!record.sourceStartedAt) return undefined
  const candidates = []
  for (const candidate of await browser.downloads.search({})) {
    if (await matches(candidate)) candidates.push(candidate)
  }
  if (candidates.length > 1) throw new Error('无法唯一恢复浏览器下载，保留原文件与交接记录')
  if (!candidates.length) return undefined
  record.downloadId = candidates[0].id
  await persistLocalDownloads()
  return candidates[0]
}

async function preserveBrowserDownload(
  item: Browser.downloads.DownloadItem,
  resource: MediaResource,
  intent?: DownloadClickIntent,
): Promise<boolean> {
  // HLS/DASH 下载的是完整媒体，不能用浏览器保存的单个清单替代。
  if (resource.kind !== 'file' && resource.kind !== 'media') return false
  const [current] = await browser.downloads.search({ id: item.id })
  if (!current || current.state === 'interrupted' && current.error === 'USER_CANCELED' && !current.canResume) {
    throw new Error('浏览器下载已取消，不再创建桌面任务')
  }
  // 原请求已经消耗过一次性地址。让浏览器保全响应，完成后复制成功文件；
  // 只有点击前交给 Core 的明确链接才重新由 Core 发起网络传输。
  await queueLocalBrowserDownload({
    downloadId: item.id, requestId: `local-file:${crypto.randomUUID()}`,
    sourceKey: await localDownloadIntentUrl(item.url), pageUrl: resource.pageUrl || '', intent,
    tabId: [resource.tabId, intent?.tabId, downloadIdentity(item).tabId].find(id => id != null && id >= 0),
    importUrl: 'data:application/octet-stream,',
  })
  return true
}

async function continueLocalDownloadHandoffs(): Promise<void> {
  if (localDownloadsPass) return
  localDownloadsPass = true
  if (localDownloadsTimer) clearTimeout(localDownloadsTimer)
  localDownloadsTimer = undefined
  try {
    await hydrateLocalDownloads()
    for (const record of localDownloadHandoffs.values()) {
      if (record.retryAt && record.retryAt > Date.now()) continue
      try {
        const item = await findLocalBrowserDownload(record)
        if (!item) {
          // 浏览器重启可能不恢复下载历史；已持久交接仍可按原身份查询，不能重发导入。
          const response = await native({ op: 'handoff_status', ...(record.handoffId
            ? { handoff_id: record.handoffId } : { client_request_id: record.requestId }) }, 2_500)
          if (!response?.ok) throw new Error('无法确认原文件交接状态')
          if (response.handoff?.id) {
            record.handoffId = String(response.handoff.id)
            if (localFileTaskReadiness(response.handoff) === 'waiting') {
              record.retryAt = Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS
              await persistLocalDownloads()
              continue
            }
          } else if (record.handoffId) throw new Error('原文件交接回执暂不可用')
          // 没有浏览器项可证明当前路径仍归其所有，保留源文件，仅结束已确认的跟踪。
          localDownloadHandoffs.delete(record.requestId)
          await persistLocalDownloads()
          continue
        }
        if (item.state === 'interrupted' && item.error === 'USER_CANCELED') {
          localDownloadHandoffs.delete(record.requestId)
          await persistLocalDownloads()
          continue
        }
        if (item.state !== 'complete' || !item.filename) continue
        if (!record.handoffId) {
          const config = await settings()
          const size = Math.max(0, item.fileSize >= 0 ? item.fileSize : item.bytesReceived)
          if (!shouldTakeover({ url: item.url, sourcePageUrl: record.pageUrl, size,
            ...config, ...record.intent, strongEvidence: true, localBrowserDownload: true })
            || (!record.intent?.ctrlForce && isHandoffSuppressed(config.suppressions, record.pageUrl, 'file'))) {
            localDownloadHandoffs.delete(record.requestId)
            await persistLocalDownloads()
            continue
          }
          // data URL 内容可能远超 Native Messaging 限制；文件字节只从已完成的本地文件复制。
          const url = record.importUrl || (item.url.startsWith('data:') ? 'data:application/octet-stream,' : item.url)
          // 登记写盘失败后记录仍可能留在内存；恢复轮询不能绕过持久身份再提交。
          // 先固定完成文件的路径，丢回执后也不能把同 ID 的另一条路径当作原件删除。
          record.sourceFilename = item.filename
          await persistLocalDownloads()
          const response = await native({ op: 'offer_local_file', resource: {
            url, local_source_path: item.filename, filename: item.filename.split(/[\\/]/).pop(),
            mime_type: item.mime, size, resource_kind: 'file', source_page_url: record.pageUrl,
            client_request_id: record.requestId, evidence: ['browser_completed_local_file'],
          } })
          // A transport failure or empty success envelope is not a rejection:
          // Core may already own the import. Retain the original request ID
          // and source file, and retry at the recovery cadence rather than
          // repeatedly submitting the same import every second.
          if (response?.transport_error || response?.ok === true && !response.handoff?.id) {
            record.retryAt = Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS
            await persistLocalDownloads()
            continue
          }
          if (!desktopAcceptedHandoff(response)) {
            localDownloadHandoffs.delete(record.requestId)
            await persistLocalDownloads()
            continue
          }
          record.handoffId = String(response.handoff.id)
          delete record.retryAt
          await persistLocalDownloads()
          await trackHandoff(record.handoffId)
        }
        const response = await native({ op: 'handoff_status', handoff_id: record.handoffId }, 2_500)
        if (!response?.ok || !response.handoff?.id) {
          record.retryAt = Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS
          await persistLocalDownloads()
          continue
        }
        const handoff = response.handoff as BrowserHandoffPayload
        const readiness = localFileTaskReadiness(handoff)
        if (readiness === 'waiting') continue
        await rememberHandoffSuppression((handoff as any).suppression)
        if (readiness === 'safe-to-remove' && !await removeBrowserDownload(item)) continue
        localDownloadHandoffs.delete(record.requestId)
        await persistLocalDownloads()
      } catch (error) {
        // 状态不明时保留源文件，并以持久 request ID 恢复查询，不能重复创建任务或删源。
        record.retryAt = Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS
        await persistLocalDownloads().catch(() => undefined)
        console.warn('HLS Downloader local file handoff awaits desktop', error)
      }
    }
  } finally {
    localDownloadsPass = false
    if (localDownloadHandoffs.size) {
      const now = Date.now()
      const delay = Math.min(...[...localDownloadHandoffs.values()].map(record => Math.max(1_000, (record.retryAt || now) - now)))
      localDownloadsTimer = setTimeout(() => {
        localDownloadsTimer = undefined
        void continueLocalDownloadHandoffs()
      }, delay)
    }
  }
}
/**
 * Chromium delivers a browser DownloadItem after the response has already
 * been accepted. Prepare request identity and policy at response time, but
 * resolve ownership in onCreated: preserve ordinary response bytes for local
 * import, or pause an adaptive manifest before offering it to the media engine.
 */
const earlyBrowserTakeovers = new Map<string, EarlyBrowserTakeover>()
const activeDownloadOffers = new Map<string, Promise<any>>()
const DIRECT_OFFERS_STORAGE_KEY = 'pending-direct-offers-v1'
const directOfferStore = new SessionListStore<{ key: string, tabId?: number, clientRequestId: string }>(browser.storage.local)

function downloadIdentity(item: Browser.downloads.DownloadItem): { tabId?: number, frameId?: number } {
  // Chrome's DownloadItem type has no tab/frame fields, but some browsers expose
  // them at runtime. URL-only fallback is disabled when tabId is absent.
  const runtimeItem = item as Browser.downloads.DownloadItem & { tabId?: unknown, frameId?: unknown }
  const { tabId, frameId } = runtimeItem
  return {
    ...(validDownloadIdentity(tabId) ? { tabId } : {}),
    ...(validDownloadIdentity(frameId) ? { frameId } : {}),
  }
}
const requestChains = new RequestChainStore()
const blobSources = new BlobSourceStore()
let nativeBridge: NativeBridge | null = null
let takeoverSettingsSync: TakeoverSettingsSync | null = null
const inspectedAdaptive = new InspectionCache()
const resourceSessionStore = new SessionListStore<MediaResource>(browser.storage.session)
let browserClientIdPromise: Promise<string> | null = null
const HANDOFF_TRACKER_STORAGE_KEY = 'handoff-tracker-v1'

interface TrackedHandoff {
  id: string
  resourceId: string
  selectionKey?: string
  tabId?: number
  browserRequestId?: string
  status: string
  checkedAt: number
  failures: number
  presentation?: string
  suppression?: unknown
  taskStatus?: string
  taskStage?: string
  taskDownloadedBytes?: number
  taskTotalBytes?: number
  taskErrorCode?: string
}

const trackedHandoffs = new Map<string, TrackedHandoff>()
const MAX_TRACKED_HANDOFFS = 128
const TRACKED_HANDOFF_RETENTION_MS = 10 * 60_000
let handoffTrackerHydrated = false
let handoffTrackerPolling = false
let handoffTrackerTimer: ReturnType<typeof setTimeout> | null = null
let lastDesktopPingAt = 0

// Paused downloads survive browser restarts, so their Core ownership records
// must outlive storage.session as well. The alarm rehydrates this local queue.
const pausedFollowUpStore = new PausedHandoffFollowUpStore({
  get: key => browser.storage.local.get(key),
  set: items => browser.storage.local.set(items),
}, PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY)
const pausedFollowUpsInFlight = new Set<string>()
let pausedFollowUpPass = false
let pausedFollowUpAccelerator: ReturnType<typeof setTimeout> | null = null
const WORKER_ALARM_NAME = 'desktop-heartbeat'
const WORKER_ALARM_PERIOD_MINUTES = 0.5
const BADGE_REFRESH_MINIMUM_MS = 5 * 60_000
let lastBadgeRefreshAt = 0

function browserClientId(): Promise<string> {
  browserClientIdPromise ||= stableBrowserClientId(
    browser.storage.local,
    () => globalThis.crypto?.randomUUID?.() || `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`,
  )
  return browserClientIdPromise
}

function extensionIdentity() {
  const navigatorWithBrave = globalThis.navigator as Navigator & { brave?: unknown }
  return {
    version: browser.runtime.getManifest().version,
    browser: detectBrowserFamily(
      browser.runtime.getURL('/'),
      navigatorWithBrave?.userAgent || '',
      Boolean(navigatorWithBrave?.brave),
    ),
  }
}

const SENSITIVE_REPLAY_KEY = /(?:cookie|authorization|token|password|secret|credential|request[_-]?headers)/i

function replayMetadata(value: Record<string, string> | undefined): Record<string, string> {
  return Object.fromEntries(
    Object.entries(value || {})
      .map(([key, raw]) => {
        const normalizedKey = String(key).trim().slice(0, 64)
        let normalizedValue = String(raw || '').trim().slice(0, 512)
        if (/url/i.test(normalizedKey)) {
          try {
            const url = new URL(normalizedValue)
            url.search = ''
            url.hash = ''
            normalizedValue = url.toString()
          } catch {}
        }
        return [normalizedKey, normalizedValue] as const
      })
      .filter(([key, value]) => Boolean(key && value) && !SENSITIVE_REPLAY_KEY.test(key))
      .slice(0, 12),
  )
}

async function settings() {
  const data = await browser.storage.local.get([
    'enabled', 'minimumBytes', 'excludedHosts', 'authorizedCookieHosts',
    HANDOFF_SUPPRESSION_STORAGE_KEY,
  ])
  return {
    enabled: data.enabled !== false,
    minimumBytes: Number(data.minimumBytes ?? 0),
    excludedHosts: Array.isArray(data.excludedHosts)
      ? data.excludedHosts.map(value => normalizeHost(String(value || ''))).filter(Boolean)
      : [],
    authorizedCookieHosts: normalizeCookiePermissionHosts(data.authorizedCookieHosts),
    suppressions: normalizeHandoffSuppressions(data[HANDOFF_SUPPRESSION_STORAGE_KEY]),
  }
}

async function clearTabResources(tabId: number, keep?: string): Promise<void> {
  const values = await browser.storage.session.get(null)
  const prefix = pageResourceKey(tabId)
  const keys = Object.keys(values).filter(key => key !== keep
    && (key === prefix || key.startsWith(`${prefix}:page:`)))
  if (keys.length) await browser.storage.session.remove(keys)
}

const responseFilename = contentDispositionFilename

const observedTabPages = new Map<number, string>()

async function topLevelPageUrl(tabId: number, fallback = ''): Promise<string> {
  if (tabId >= 0) {
    const tabUrl = (await browser.tabs.get(tabId).catch(() => null))?.url || ''
    if (/^https?:\/\//i.test(tabUrl)) {
      // URL events own later updates; an older async lookup must not undo them.
      if (!observedTabPages.has(tabId)) observedTabPages.set(tabId, pageIdentity(tabUrl))
      return tabUrl
    }
  }
  return fallback
}

async function rememberHandoffSuppression(value: unknown): Promise<void> {
  const rule = normalizeHandoffSuppressions([value], 1)[0]
  if (!rule) return
  const stored = await browser.storage.local.get(HANDOFF_SUPPRESSION_STORAGE_KEY)
  const current = normalizeHandoffSuppressions(stored[HANDOFF_SUPPRESSION_STORAGE_KEY])
  const next = [rule, ...current.filter(item => item.host !== rule.host || item.kind !== rule.kind)]
  await browser.storage.local.set({
    [HANDOFF_SUPPRESSION_STORAGE_KEY]: normalizeHandoffSuppressions(next),
  })
}

async function hydrateHandoffTracker(): Promise<void> {
  if (handoffTrackerHydrated) return
  handoffTrackerHydrated = true
  const stored: Record<string, unknown> = await browser.storage.session
    .get(HANDOFF_TRACKER_STORAGE_KEY)
    .catch(() => ({})) as Record<string, unknown>
  const values = Array.isArray(stored[HANDOFF_TRACKER_STORAGE_KEY]) ? stored[HANDOFF_TRACKER_STORAGE_KEY] : []
  for (const value of values) {
    if (!value || typeof value !== 'object') continue
    const item = value as Partial<TrackedHandoff>
    const id = String(item.id || '')
    if (!id) continue
    trackedHandoffs.set(id, {
      id,
      resourceId: String(item.resourceId || ''),
      selectionKey: typeof item.selectionKey === 'string' ? item.selectionKey : undefined,
      tabId: validDownloadIdentity(item.tabId) ? item.tabId : undefined,
      browserRequestId: typeof item.browserRequestId === 'string' ? item.browserRequestId : undefined,
      status: String(item.status || 'pending'),
      checkedAt: Number(item.checkedAt || 0),
      failures: Math.max(0, Number(item.failures || 0)),
      presentation: typeof item.presentation === 'string' ? item.presentation : undefined,
      suppression: item.suppression,
      taskStatus: typeof item.taskStatus === 'string' ? item.taskStatus : undefined,
      taskStage: typeof item.taskStage === 'string' ? item.taskStage : undefined,
      taskDownloadedBytes: Number.isFinite(Number(item.taskDownloadedBytes)) ? Math.max(0, Number(item.taskDownloadedBytes)) : undefined,
      taskTotalBytes: Number.isFinite(Number(item.taskTotalBytes)) ? Math.max(0, Number(item.taskTotalBytes)) : undefined,
      taskErrorCode: typeof item.taskErrorCode === 'string' ? item.taskErrorCode : undefined,
    })
  }
  pruneTrackedHandoffs()
}

function updateTrackedHandoff(target: TrackedHandoff, handoff: BrowserHandoffPayload): void {
  target.status = String(handoff.status || target.status || 'pending')
  target.checkedAt = Date.now()
  target.failures = 0
  target.presentation = typeof handoff.presentation === 'string' ? handoff.presentation : target.presentation
  target.suppression = (handoff as BrowserHandoffPayload & { suppression?: unknown }).suppression
  target.taskStatus = typeof handoff.task_status === 'string' ? handoff.task_status : target.taskStatus
  target.taskStage = typeof handoff.task_stage === 'string' ? handoff.task_stage : target.taskStage
  target.taskDownloadedBytes = Number.isFinite(Number(handoff.task_downloaded_bytes))
    ? Math.max(0, Number(handoff.task_downloaded_bytes))
    : target.taskDownloadedBytes
  target.taskTotalBytes = Number.isFinite(Number(handoff.task_total_bytes))
    ? Math.max(0, Number(handoff.task_total_bytes))
    : target.taskTotalBytes
  target.taskErrorCode = typeof handoff.task_error_code === 'string' ? handoff.task_error_code : target.taskErrorCode
}

function pruneTrackedHandoffs(now = Date.now()): void {
  for (const [id, item] of trackedHandoffs) {
    if (handoffTerminalStatus(item.status) && item.checkedAt > 0
      && now - item.checkedAt > TRACKED_HANDOFF_RETENTION_MS) {
      trackedHandoffs.delete(id)
    }
  }
  if (trackedHandoffs.size <= MAX_TRACKED_HANDOFFS) return
  const candidates = [...trackedHandoffs.entries()].sort((left, right) => {
    const leftTerminal = handoffTerminalStatus(left[1].status) ? 0 : 1
    const rightTerminal = handoffTerminalStatus(right[1].status) ? 0 : 1
    return leftTerminal - rightTerminal
      || (left[1].checkedAt || 0) - (right[1].checkedAt || 0)
  })
  for (const [id] of candidates.slice(0, trackedHandoffs.size - MAX_TRACKED_HANDOFFS)) {
    trackedHandoffs.delete(id)
  }
}

async function persistHandoffTracker(): Promise<void> {
  pruneTrackedHandoffs()
  const values = [...trackedHandoffs.values()].slice(-24)
  await browser.storage.session.set({ [HANDOFF_TRACKER_STORAGE_KEY]: values }).catch(() => undefined)
}

function scheduleHandoffPoll(delay = 120): void {
  if (handoffTrackerTimer) return
  handoffTrackerTimer = setTimeout(() => {
    handoffTrackerTimer = null
    void pollTrackedHandoffs()
  }, delay)
}

async function trackHandoff(handoffId: string, resourceId = '', tabId?: number, selectionKey?: string): Promise<TrackedHandoff> {
  await hydrateHandoffTracker()
  const id = String(handoffId || '')
  const current = trackedHandoffs.get(id)
  if (current) {
    if (resourceId && !current.resourceId) current.resourceId = resourceId
    if (selectionKey) current.selectionKey = selectionKey
    if (validDownloadIdentity(tabId)) current.tabId = tabId
    scheduleHandoffPoll()
    return current
  }
  const tracked: TrackedHandoff = { id, resourceId, selectionKey, tabId, status: 'pending', checkedAt: 0, failures: 0 }
  trackedHandoffs.set(id, tracked)
  pruneTrackedHandoffs()
  scheduleHandoffPoll()
  await persistHandoffTracker()
  return tracked
}

async function pollTrackedHandoffs(): Promise<void> {
  if (handoffTrackerPolling) return
  await hydrateHandoffTracker()
  const next = [...trackedHandoffs.values()]
    .filter(item => !handoffTerminalStatus(item.status)
      && (!item.failures || Date.now() - item.checkedAt >= PAUSED_FOLLOW_UP_RECHECK_MS))
    .sort((left, right) => left.checkedAt - right.checkedAt)[0]
  if (!next) return
  handoffTrackerPolling = true
  try {
    const response = await native({ op: 'handoff_status', handoff_id: next.id }, 2_500)
    if (!response?.ok || !response.handoff?.id) throw new Error(response?.error || '无法读取下载确认状态')
    const handoff = response.handoff
    const status = String(handoff?.status || 'pending')
    updateTrackedHandoff(next, handoff || {})
    if (handoffTerminalStatus(status)) await rememberHandoffSuppression(handoff?.suppression)
  } catch {
    next.checkedAt = Date.now()
    next.failures += 1
    next.status = 'connection_lost'
  } finally {
    handoffTrackerPolling = false
    pruneTrackedHandoffs()
    await persistHandoffTracker()
    const pending = [...trackedHandoffs.values()].filter(item => !handoffTerminalStatus(item.status))
    if (pending.length) {
      const now = Date.now()
      scheduleHandoffPoll(Math.min(...pending.map(item => item.failures
        ? Math.max(0, item.checkedAt + PAUSED_FOLLOW_UP_RECHECK_MS - now) : 900)))
    }
  }
}

async function handoffStatus(handoffId: string): Promise<{ ok: true, handoff: TrackedHandoff }> {
  const handoff = await trackHandoff(handoffId)
  return { ok: true, handoff: { ...handoff } }
}

async function waitForHandoffResolution(handoffId: string): Promise<TrackedHandoff> {
  const deadline = Date.now() + 125_000
  await trackHandoff(handoffId)
  while (Date.now() < deadline) {
    const current = await handoffStatus(handoffId)
    if (handoffTerminalStatus(current.handoff.status)) return current.handoff
    await new Promise(resolve => setTimeout(resolve, 450))
  }
  // This is only a local polling timeout, not a Core-confirmed expiry. Keep it
  // non-terminal so browser recovery cannot race an accepted desktop task.
  return { id: handoffId, resourceId: '', status: 'connection_lost', checkedAt: Date.now(), failures: 0 }
}

function schedulePausedFollowUpAccelerator(delay: number): void {
  if (pausedFollowUpAccelerator) return
  pausedFollowUpAccelerator = setTimeout(() => {
    pausedFollowUpAccelerator = null
    void continuePausedHandoffFollowups()
  }, Math.max(0, delay))
}

async function rememberPausedHandoffFollowUp(downloadId: number, handoffId: string): Promise<PausedHandoffFollowUp> {
  // 只在桌面确认接收后创建；先保存清理责任，避免 worker 休眠留下暂停副本。
  const record = await pausedFollowUpStore.remember({
    downloadId,
    handoffId,
    phase: 'cleanup',
    deadline: Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS,
  })
  schedulePausedFollowUpAccelerator(PAUSED_FOLLOW_UP_RECHECK_MS)
  return record
}

async function resumeFollowUpDownload(downloadId: number): Promise<boolean> {
  let current: Browser.downloads.DownloadItem | undefined
  try {
    const [found] = await browser.downloads.search({ id: downloadId })
    current = found
  } catch {
    return false
  }
  if (!current) return true
  return resumeBrowserDownload(current, true)
}

async function removeFollowUpDownload(downloadId: number): Promise<boolean> {
  let current: Browser.downloads.DownloadItem | undefined
  try {
    const [found] = await browser.downloads.search({ id: downloadId })
    current = found
  } catch {
    return false
  }
  if (!current) return true
  return removeBrowserDownload(current)
}

async function retryPausedFollowUp(record: PausedHandoffFollowUp): Promise<void> {
  await pausedFollowUpStore.remember({
    ...record,
    deadline: Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS,
  })
  schedulePausedFollowUpAccelerator(PAUSED_FOLLOW_UP_RECHECK_MS)
}

/**
 * Continue the lifecycle of paused browser downloads left behind by a takeover.
 * storage.local plus this pass are the reliable carriers across service-worker
 * suspension and browser restart; bare setTimeout only accelerates the alive-worker case. A record
 * stays until the desktop proves success (remove the browser copy) or failure
 * (resume it) — uncertainty keeps the item visibly paused for the user.
 */
async function continuePausedHandoffFollowups(): Promise<void> {
  if (pausedFollowUpPass) return
  pausedFollowUpPass = true
  try {
    await pausedFollowUpStore.hydrate()
    for (const record of pausedFollowUpStore.list()) {
      if (record.deadline > Date.now()) continue
      const followUpKey = record.clientRequestId || record.handoffId
      if (pausedFollowUpsInFlight.has(followUpKey)) continue
      pausedFollowUpsInFlight.add(followUpKey)
      try {
        if (record.phase === 'cleanup') {
          const removed = await removeFollowUpDownload(record.downloadId)
          if (removed) await pausedFollowUpStore.drop(followUpKey)
          else await retryPausedFollowUp(record)
          continue
        }
        // The user may resume, cancel, finish or erase the browser item while
        // Core is unreachable. That is an explicit browser-side ownership
        // decision; never resume it again or remove its file on a later alarm.
        let browserItems: Browser.downloads.DownloadItem[]
        try {
          browserItems = await browser.downloads.search({ id: record.downloadId })
        } catch {
          await retryPausedFollowUp(record)
          continue
        }
        const [browserItem] = browserItems
        if (!browserItem || (browserItem.paused !== true && browserItem.state !== 'complete')) {
          await pausedFollowUpStore.drop(followUpKey)
          continue
        }
        let handoff: BrowserHandoffPayload = {}
        try {
          const response = await native({ op: 'handoff_status', ...(record.clientRequestId
            ? { client_request_id: record.clientRequestId } : { handoff_id: record.handoffId }) }, 2_500)
          if (response?.ok) handoff = (response.handoff || {}) as BrowserHandoffPayload
        } catch {
          // An unreachable desktop must not resume or remove anything.
        }
        const step = stepPausedHandoffFollowUp(record, handoff, Date.now(), PAUSED_FOLLOW_UP_RECHECK_MS)
        if (step.kind === 'keep-paused') {
          await pausedFollowUpStore.remember(step.followUp)
          continue
        }
        if (step.kind === 'resume-download') {
          const resumed = await resumeFollowUpDownload(record.downloadId)
          if (resumed) await pausedFollowUpStore.drop(followUpKey)
          else await retryPausedFollowUp(record)
          continue
        }
        const removed = await removeBrowserDownload(browserItem, true)
        if (removed) await pausedFollowUpStore.drop(followUpKey)
        else await retryPausedFollowUp(record)
      } finally {
        pausedFollowUpsInFlight.delete(followUpKey)
      }
    }
  } finally {
    pausedFollowUpPass = false
  }
  // Every phase uses its earliest persisted deadline while the worker stays
  // alive; the alarm covers suspension.
  const parked = pausedFollowUpStore.list()[0]
  if (parked) schedulePausedFollowUpAccelerator(parked.deadline - Date.now())
}

async function saveResource(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1, expectedPageUrl?: string) {
  const kind = resource.kind || classifyResource(resource.url, resource.mimeType)
  if (!kind) return
  let pageUrl = resource.pageUrl || ''
  if (tabId >= 0) {
    // A media player may live in a CDN iframe. The user-facing source page is
    // still the browser tab URL, not the iframe/media host. This gives the
    // handoff a stable Referer fallback and prevents an iframe URL from
    // becoming the incorrectly advertised source page.
    pageUrl = await topLevelPageUrl(tabId, pageUrl)
  }
  // Async inspection must not adopt a newly navigated tab's page identity.
  if (expectedPageUrl !== undefined && pageIdentity(pageUrl) !== pageIdentity(expectedPageUrl)) return false
  const key = pageResourceKey(tabId, pageUrl)
  const merged = await resourceSessionStore.update(key, resources => mergeResources(
    resources,
    { ...resource, pageUrl, kind, id: resourceId(resource.url), seenAt: Date.now() },
    100,
    true,
  ))
  await setResourceBadge(tabId, merged)
  return true
}

function badgeText(resources: MediaResource[]): string {
  const count = compactResources(resources.filter(resource => resource.evidence?.includes('playback_confirmed')), 100).length
  return count ? String(Math.min(99, count)) : ''
}

async function setResourceBadge(tabId: number, resources: MediaResource[]): Promise<void> {
  if (tabId < 0) return
  await browser.action.setBadgeText({ tabId, text: badgeText(resources) }).catch(() => undefined)
}

/**
 * The popup must never be responsible for clearing the red counter.  An MV3
 * worker can be resumed long after an inactive tab's page has changed, so
 * reload the page-scoped cache on activation and drop expired observations.
 */
async function refreshTabBadge(tabId: number): Promise<void> {
  if (tabId < 0) return
  const pageUrl = await topLevelPageUrl(tabId)
  if (!/^https?:\/\//i.test(pageUrl)) {
    await setResourceBadge(tabId, [])
    return
  }
  const key = pageResourceKey(tabId, pageUrl)
  const resources = await resourceSessionStore.update(
    key,
    current => compactResources(pruneExpiredResources(current), 100, true),
  )
  await setResourceBadge(tabId, resources)
}

async function refreshOpenTabBadges(): Promise<void> {
  const tabs = await browser.tabs.query({}).catch(() => [])
  await Promise.all(tabs
    .map(tab => Number(tab.id))
    .filter(tabId => Number.isInteger(tabId) && tabId >= 0)
    .map(tabId => refreshTabBadge(tabId)))
}

function refreshOpenTabBadgesWhenDue(): void {
  // Badge expiry stays on the slow cadence the dedicated alarm used to run
  // with; the shared worker alarm only checks the clock here.
  const now = Date.now()
  if (now - lastBadgeRefreshAt < BADGE_REFRESH_MINIMUM_MS) return
  lastBadgeRefreshAt = now
  void refreshOpenTabBadges().catch(error => console.warn('HLS Downloader badge refresh failed', error))
}

async function sendCapturedResource(tabId: number, resource: Omit<MediaResource, 'id' | 'seenAt'>): Promise<void> {
  if (tabId < 0) return
  const message = { type: 'captured-resource', resource }
  const frameId = Number(resource.frameId)
  if (Number.isInteger(frameId) && frameId >= 0) {
    await browser.tabs.sendMessage(tabId, message, { frameId }).catch(() => undefined)
    return
  }
  await browser.tabs.sendMessage(tabId, message).catch(() => undefined)
}

async function srcdocPartitionKeyForFrame(
  tabId: number,
  frameId: number,
  getPartitionKey: (details: { tabId: number, frameId: number }) => Promise<{ partitionKey: { topLevelSite?: string } }>,
): Promise<{ topLevelSite: string, hasCrossSiteAncestor: boolean } | undefined> {
  const context = await browser.tabs.sendMessage(
    tabId,
    { type: 'srcdoc-partition-context' },
    { frameId },
  ).catch(() => undefined) as SrcdocFrameContext | undefined
  if (!context || typeof context.href !== 'string' || typeof context.origin !== 'string'
    || context.href.split(/[?#]/, 1)[0] !== 'about:srcdoc'
    || !Array.isArray(context.ancestorOrigins) || context.ancestorOrigins.some(origin => typeof origin !== 'string')) return undefined

  let topLevel: { partitionKey: { topLevelSite?: string } }
  try {
    topLevel = await getPartitionKey({ tabId, frameId: 0 })
  } catch (error) {
    if (isCookieHostPermissionError(error)) return undefined
    throw error
  }
  return cookiePartitionKeyForSrcdoc(topLevel.partitionKey?.topLevelSite || '', context)
}

async function cookieRecordsFor(url: string, pageUrl = '', tabId = -1, frameId = 0, allPaths = false): Promise<ReplayCookie[]> {
  const cookieUrl = cookieLookupUrl(url)
  if (!cookieUrl) return []
  const config = await settings()
  if (!cookiePermissionAllows(cookieUrl, pageUrl, config.authorizedCookieHosts)) return []
  // 容器和隐私窗口必须使用发起媒体请求的标签页 Cookie store。
  const stores = tabId >= 0 ? await browser.cookies.getAllCookieStores() : []
  const storeId = stores.find(store => store.tabIds.includes(tabId))?.id
  if (tabId >= 0 && !storeId) throw new Error('无法读取当前标签页的 Cookie 存储；请重新打开网页后重试')
  type PartitionKey = { topLevelSite?: string, hasCrossSiteAncestor?: boolean }
  const cookieApi = browser.cookies as typeof browser.cookies & {
    getPartitionKey?: (details: { tabId: number, frameId: number }) => Promise<{ partitionKey: PartitionKey }>
  }
  const fallbackSite = cookiePartitionSite(pageUrl)
  let partitionKey: PartitionKey | undefined
  if (tabId >= 0 && cookieApi.getPartitionKey) {
    try {
      partitionKey = (await cookieApi.getPartitionKey({ tabId, frameId })).partitionKey
    } catch (error) {
      if (frameId === 0 || !isSrcdocCookieHostPermissionError(error)) throw error
      partitionKey = await srcdocPartitionKeyForFrame(
        tabId,
        frameId,
        details => cookieApi.getPartitionKey!(details),
      )
    }
  } else if (fallbackSite) {
    partitionKey = { topLevelSite: fallbackSite }
  }
  const target = new URL(cookieUrl)
  const queries: Array<{ url?: string, domain?: string }> = [{ url: cookieUrl }]
  if (allPaths) {
    // 清单未播放到的分片/密钥也需要自己的 Path Cookie；只导出目标主机适用的记录。
    const labels = target.hostname.split('.')
    for (let i = 0; i === 0 || (i < labels.length - 1 && !/^\d+$/.test(labels[i])); i += 1) {
      queries.push({ domain: labels.slice(i).join('.') })
    }
  }
  const collected = new Map<string, ReplayCookie>()
  for (const query of queries) {
    const base = { ...query, ...(storeId ? { storeId } : {}) }
    const values = await browser.cookies.getAll(base)
    const partitioned = partitionKey?.topLevelSite
      ? await browser.cookies.getAll({ ...base, partitionKey }) : []
    for (const cookie of [...values.filter(cookie => !cookie.partitionKey?.topLevelSite), ...partitioned]) {
      const record: ReplayCookie = {
        name: cookie.name, value: cookie.value, domain: cookie.domain, path: cookie.path,
        hostOnly: cookie.hostOnly, secure: cookie.secure,
        ...(cookie.expirationDate === undefined ? {} : { expirationDate: cookie.expirationDate }),
        ...(cookie.partitionKey ? { partitionKey: cookie.partitionKey } : {}),
      }
      // 按每条 Cookie 的 Path 构造 URL，仅检查 Domain/Secure/有效期；发送时再匹配实际 Path。
      if (!cookieMatchesUrl(record, allPaths ? target.origin + record.path : cookieUrl)) continue
      collected.set(replayCookieIdentity(record), record)
    }
  }
  const result = [...collected.values()]
  if (result.length > 256 || JSON.stringify(result).length > 128 * 1024) {
    throw new Error('当前站点 Cookie 超出下载上下文限制；请清理本站过期登录数据后重试')
  }
  return result
}

async function cookiesFor(url: string, pageUrl = '', tabId = -1, frameId = 0): Promise<string> {
  return cookieHeaderForUrl(await cookieRecordsFor(url, pageUrl, tabId, frameId), url)
}

async function native(message: Record<string, unknown>, timeoutMs?: number): Promise<any> {
  if (!nativeBridge) return Promise.reject(new Error('插件连接尚未初始化'))
  const identity = extensionIdentity()
  const operation = String(message.op || '')
  const retryCount = new Set([
    'ping', 'offer', 'recover_offer', 'offer_local_file', 'handoff_status', 'media_push_status',
  ]).has(operation) ? 1 : 0
  const payload = {
    ...message,
    version: identity.version,
    client_id: await browserClientId(),
    browser: identity.browser,
  }
  return nativeBridge.request(payload, timeoutMs, retryCount)
}

async function pingDesktop(): Promise<any> {
  const response = takeoverSettingsSync
    ? await takeoverSettingsSync.applyPing(await native({ op: 'ping' }))
    : await native({ op: 'ping' })
  if (response?.ok) lastDesktopPingAt = Date.now()
  return response
}

// Abort both pending probes and response body reads when their page disappears.
const adaptiveInspectionPages = new Map<number, AbortController>()
const activeAdaptiveInspections = new Map<string, AbortController>()

async function inspectAdaptive(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1): Promise<void> {
  // Only inspect page-owned GET responses. Extension probes must not recursively
  // trigger more probes, and a POST manifest must never be reissued as a GET.
  if (tabId < 0 || (resource.method && resource.method.toUpperCase() !== 'GET')
    || (resource.statusCode !== undefined
      && (resource.statusCode < 200 || resource.statusCode >= 300 || [204, 205].includes(resource.statusCode)))) return
  const pageController = adaptiveInspectionPages.get(tabId) || new AbortController()
  adaptiveInspectionPages.set(tabId, pageController)
  let inspectionController: AbortController | undefined
  const isCurrentPage = () => !pageController.signal.aborted && !inspectionController?.signal.aborted
    && adaptiveInspectionPages.get(tabId) === pageController
  const normalized = {
    ...resource,
    url: canonicalMediaUrl(resource.url, resource.kind),
  }
  if (!['hls', 'dash'].includes(normalized.kind)) return
  const inspectionKey = await adaptiveInspectionKey(normalized, tabId)
  if (!isCurrentPage()) return
  if (resource.inspected) {
    // A master without a known child still needs the bounded rendition probe.
    if (resource.kind !== 'hls' || resource.manifestType === 'media' || resource.isLive !== undefined) {
      activeAdaptiveInspections.get(inspectionKey)?.abort()
      inspectedAdaptive.claim(inspectionKey)
    }
    return
  }
  if (!inspectedAdaptive.claim(inspectionKey)) return
  if (activeAdaptiveInspections.size >= 8) {
    inspectedAdaptive.defer(inspectionKey, 1_000)
    return
  }
  inspectionController = new AbortController()
  activeAdaptiveInspections.set(inspectionKey, inspectionController)
  try {
    const sourcePageUrl = await topLevelPageUrl(tabId, resource.pageUrl || '')
    if (!isCurrentPage()) return
    const probeResource = {
      ...normalized,
      signal: AbortSignal.any([pageController.signal, inspectionController.signal]),
      requestContextForUrl: async (url: string) => {
        if (!isCurrentPage()) throw new Error('媒体来源页面已变化')
        const chain = requestChains.find({ url, referrer: resource.pageUrl, frameId: resource.frameId,
          ...(url === normalized.url ? { requestId: resource.replayContext?.request_id, responseIdentity: resource.responseIdentity } : {}),
        }, Date.now(), tabId, true, 'exact')
        if (chain && chain.method.toUpperCase() !== 'GET') throw new Error('不能重放非 GET 清单请求')
        const sameOrigin = new URL(url).origin === new URL(normalized.url).origin
        const requestHeaders = chain?.requestHeaders || (sameOrigin ? normalized.requestHeaders : undefined)
        // Captured empty Referer/Origin is authoritative. Only unobserved
        // requests use the source frame as a fallback, never another CDN's token.
        const identity = requestHeaders !== undefined
          ? capturedRequestIdentity(requestHeaders)
          : resourceRequestIdentity({ pageUrl: resource.pageUrl })
        const inspectionHeaders: Record<string, string> = {}
        if (identity.referer) inspectionHeaders.referer = identity.referer
        if (identity.origin) inspectionHeaders.origin = identity.origin
        const cookie = await cookiesFor(url, sourcePageUrl, tabId, resource.frameId ?? 0)
        if (cookie) inspectionHeaders.cookie = cookie
        if (!isCurrentPage()) throw new Error('媒体来源页面已变化')
        return { requestHeaders, inspectionHeaders }
      },
    }
    const metadata = normalized.kind === 'hls'
      ? await inspectHlsResource(probeResource)
      : await inspectDashResource(probeResource)
    // An old response must not be relabelled with the tab's new source page.
    if (!isCurrentPage() || pageIdentity(await topLevelPageUrl(tabId, sourcePageUrl)) !== pageIdentity(sourcePageUrl) || !isCurrentPage()) return
    if (metadata) {
      const enriched = {
        ...normalized,
        ...metadata,
        evidence: [...new Set([...(normalized.evidence || []), 'manifest_inspection'])].slice(-16),
        owner: normalized.owner || 'manifest',
        confidence: Math.max(boundedConfidence(normalized.confidence), 0.96),
        replayContext: { ...(normalized.replayContext || {}), method: 'GET' },
      }
      if (!isCurrentPage() || !await saveResource(enriched, tabId, sourcePageUrl)) return
      if (!isCurrentPage() || pageIdentity(await topLevelPageUrl(tabId, sourcePageUrl)) !== pageIdentity(sourcePageUrl) || !isCurrentPage()) return
      await sendCapturedResource(tabId, enriched)
    } else {
      // A response can be temporarily unauthorized, truncated or not yet a
      // real manifest. Back off to avoid doubling every live poll, but do not
      // keep the URL blind for the full success TTL.
      inspectedAdaptive.defer(inspectionKey)
    }
  } catch {
    // A transient CDN/auth failure must not permanently suppress inspection.
    // The captured manifest remains downloadable and the next observation can retry.
    if (isCurrentPage()) inspectedAdaptive.defer(inspectionKey)
  } finally {
    if (activeAdaptiveInspections.get(inspectionKey) === inspectionController) {
      activeAdaptiveInspections.delete(inspectionKey)
    }
  }
}

function successfulChainForResource(resource: MediaResource, explicitChain?: RequestChain): RequestChain | undefined {
  const isMatchingSuccess = (candidate: RequestChain | undefined): candidate is RequestChain => {
    if (!candidate || candidate.statusCode < 200 || candidate.statusCode >= 400) return false
    const finalUrl = canonicalMediaUrl(candidate.finalUrl, resource.kind)
    return resourceFingerprint({ url: finalUrl, kind: resource.kind }) === resourceFingerprint(resource)
  }
  if (isMatchingSuccess(explicitChain)) return explicitChain
  const found = resource.tabId !== undefined && resource.tabId >= 0
    ? requestChains.find({ url: resource.url, referrer: resource.pageUrl || '', frameId: resource.frameId,
      requestId: resource.replayContext?.request_id, responseIdentity: resource.responseIdentity }, Date.now(), resource.tabId, true)
    : requestChains.find({ url: resource.url, referrer: resource.pageUrl || '', frameId: resource.frameId,
      requestId: resource.replayContext?.request_id, responseIdentity: resource.responseIdentity }, Date.now(), undefined, true)
  return isMatchingSuccess(found) ? found : undefined
}

async function readyResourceChain(resource: MediaResource, explicitChain?: RequestChain): Promise<RequestChain | undefined> {
  let chain = successfulChainForResource(resource, explicitChain)
  if (!usesShortLivedMediaSignature(resource)) return chain
  // A Performance/media-element observation may arrive before the browser has
  // received its signed response. Do not hand the desktop a raw s/e/_t URL in
  // that gap: wait briefly for the real request and use its latest signature.
  for (let attempt = 0; attempt < 12; attempt += 1) {
    if (chain && isShortLivedMediaSignatureUsable({ ...resource, url: chain.finalUrl })) return chain
    if (attempt < 11) await new Promise(resolve => setTimeout(resolve, 125))
    chain = successfulChainForResource({ ...resource, responseIdentity: undefined,
      replayContext: { ...resource.replayContext, request_id: '' } })
  }
  throw new Error('浏览器尚未获得可用的短效签名媒体链接；请让视频继续播放或刷新页面后重试')
}

interface ResourceRequestOptions {
  allowUnverified?: boolean
  browserDownload?: DownloadLike
  browserRequestId?: string
  desktopReady?: boolean
  resourceCookies?: ReplayCookie[]
}

async function resourceRequest(
  resource: MediaResource,
  explicitChain?: RequestChain,
  options: ResourceRequestOptions = {},
) {
  resource = { ...resource, url: canonicalMediaUrl(resource.url, resource.kind) }
  const pageUrl = await topLevelPageUrl(resource.tabId ?? -1, resource.pageUrl || '')
  const requireSuccessfulRequest = resource.kind !== 'magnet'
  const findChain = () => {
    if (options.browserDownload) {
      return requestChains.find(options.browserDownload, Date.now(), resource.tabId, requireSuccessfulRequest, 'exact')
    }
    const found = resource.tabId !== undefined && resource.tabId >= 0
      ? requestChains.find({ url: resource.url, referrer: pageUrl, frameId: resource.frameId,
        requestId: resource.replayContext?.request_id, responseIdentity: resource.responseIdentity }, Date.now(), resource.tabId, requireSuccessfulRequest)
      : requestChains.find({ url: resource.url, referrer: pageUrl, frameId: resource.frameId,
        requestId: resource.replayContext?.request_id, responseIdentity: resource.responseIdentity }, Date.now(), undefined, requireSuccessfulRequest)
    return !resource.method || found?.method.toUpperCase() === resource.method.toUpperCase() ? found : undefined
  }
  let chain = !options.browserDownload && usesShortLivedMediaSignature(resource)
    ? await readyResourceChain({ ...resource, pageUrl }, explicitChain)
    : explicitChain || findChain()
  // 点击前接管尚未发出 GET；同 URL 的旧 POST 不能成为这次链接点击的请求体。
  if (resource.method && chain?.method.toUpperCase() !== resource.method.toUpperCase()) chain = undefined
  if (!chain && (!options.allowUnverified || Object.keys(resource.responseIdentity || {}).length) && resource.kind !== 'magnet') {
    // A visible URL is not by itself a browser request. Replaying a page
    // controller or stale signed gateway without a successful response chain
    // is how HTML pages became download tasks. Close the short observation
    // race, then fail closed for ordinary media/files.
    for (let attempt = 0; !chain && attempt < 8; attempt += 1) {
      await new Promise(resolve => setTimeout(resolve, 75))
      chain = findChain()
    }
    if (!chain && Object.keys(resource.responseIdentity || {}).length) {
      throw new Error('无法唯一关联该媒体响应与浏览器请求；请继续播放后重试')
    }
    const successfulResponse = resource.statusCode !== undefined
      && resource.statusCode >= 200 && resource.statusCode < 400
    if (!chain && !successfulResponse && resource.kind !== 'hls' && resource.kind !== 'dash' && resource.inspected !== true) {
      throw new Error('浏览器尚未确认该媒体请求；请继续播放或重新点击下载')
    }
  }
  return { chain, pageUrl }
}

async function resourcePayload(
  resource: MediaResource,
  explicitChain?: RequestChain,
  options: ResourceRequestOptions = {},
) {
  resource = { ...resource, url: canonicalMediaUrl(resource.url, resource.kind) }
  const { chain, pageUrl } = await resourceRequest(resource, explicitChain, options)
  const pageChain = resource.tabId !== undefined && resource.tabId >= 0
    ? requestChains.pageContext(resource.tabId, pageUrl)
    : undefined
  if (chain) {
    const freshUrl = canonicalMediaUrl(chain.finalUrl, resource.kind)
    if (resourceFingerprint({ url: freshUrl, kind: resource.kind }) === resourceFingerprint(resource)) {
      // LL-HLS and signed files keep refreshing while the selection panel is
      // open. Always hand off the most recently successful browser request,
      // not a stale Performance/fetch observation rendered by the page.
      resource = { ...resource, url: freshUrl, seenAt: Math.max(resource.seenAt || 0, chain.updatedAt) }
    }
  }
  // The source-page request is the stable default.  Individual media/CDN
  // chains remain in request_contexts and take precedence for their origin.
  const pageIdentity = resourceRequestIdentity({
    pageUrl,
    requestHeaders: pageChain?.requestHeaders || resource.requestHeaders,
  }, navigator.userAgent)
  const identity = resourceRequestIdentity(resource, navigator.userAgent)
  const chainIdentity = resourceRequestIdentity({
    pageUrl,
    requestHeaders: chain?.requestHeaders || resource.requestHeaders,
  }, navigator.userAgent)
  // The browser address bar is the source page advertised to the desktop app.
  // A main-frame navigation's Referer is the page *before* the current page,
  // so never use it as the media request's default.
  const sourceIdentity = {
    referer: pageUrl || chainIdentity.referer || pageIdentity.referer || identity.referer,
    origin: chainIdentity.origin || pageIdentity.origin || identity.origin,
    userAgent: pageIdentity.userAgent || chainIdentity.userAgent || identity.userAgent,
  }
  const requestContexts: Record<string, Record<string, unknown>> = {}
  let resourceCookie: string | undefined
  if (resource.tabId !== undefined && resource.tabId >= 0) {
    const cookieSnapshots = new Map<string, Promise<ReplayCookie[]>>()
    const addRequestContext = async (requestUrl: string, requestHeaders: Record<string, string> | undefined) => {
      let origin = ''
      try { origin = new URL(requestUrl).origin } catch {}
      if (!origin) return
      const scopedIdentity = capturedRequestIdentity(requestHeaders, navigator.userAgent)
      let snapshot = cookieSnapshots.get(origin)
      if (!snapshot) {
        snapshot = origin === new URL(resource.url).origin && options.resourceCookies !== undefined
          ? Promise.resolve(options.resourceCookies)
          : cookieRecordsFor(requestUrl, pageUrl, resource.tabId, resource.frameId ?? 0, true)
        cookieSnapshots.set(origin, snapshot)
      }
      const cookies = await snapshot
      if (origin === new URL(resource.url).origin) resourceCookie = cookieHeaderForUrl(cookies, resource.url)
      const context = {
        request_headers: replayableRequestHeaders(requestHeaders),
        referer: scopedIdentity.referer,
        origin: scopedIdentity.origin,
        user_agent: scopedIdentity.userAgent,
      }
      const pathname = new URL(requestUrl).pathname
      const path = pathname.slice(0, pathname.lastIndexOf('/') + 1)
      requestContexts[origin] = {
        ...context,
        cookie: cookieHeaderForUrl(cookies, requestUrl), cookies,
        path_contexts: { ...(requestContexts[origin]?.path_contexts as Record<string, unknown> || {}), [path]: context },
        url_contexts: { ...(requestContexts[origin]?.url_contexts as Record<string, unknown> || {}), [requestUrl.split('#', 1)[0]]: context },
      }
    }
    // 普通文件只重放自身请求；其他媒体/CDN 的身份只供 HLS/DASH 分片使用。
    const contexts = resource.kind === 'hls' || resource.kind === 'dash'
      ? requestChains.contextsForPage(resource.tabId, resource.pageUrl || pageUrl,
        Date.now(), 32, chain?.frameId ?? resource.frameId)
      : []
    // 最后应用所选媒体；其他音轨/密钥保留精确身份，目录默认使用最新请求。
    for (const context of contexts.filter(context => context.requestId !== chain?.requestId).reverse()) {
      await addRequestContext(context.finalUrl, context.requestHeaders)
    }
    if (chain) await addRequestContext(chain.finalUrl, chain.requestHeaders)
    if (!chain) {
      // Variant URLs parsed from a master HLS playlist are often chosen before
      // the browser fetches them. Reuse that master request's captured headers
      // only for the selected variant's origin, and fetch cookies for that URL.
      await addRequestContext(resource.url, resource.requestHeaders || {
        referer: resource.pageUrl || pageUrl,
        'user-agent': navigator.userAgent,
      })
    }
  }
  const replay = replayablePostRequest(chain)
  const extension = extensionIdentity()
  if (String(resource.method || '').toUpperCase() === 'POST' && !replay.request_body) {
    throw new Error('此 POST 下载包含无法安全重放的请求体；请让浏览器完成本次下载后再导入文件')
  }
  return {
    url: resource.url,
    filename: suggestedResourceFilename(resource),
    title: resource.title || '',
    mime_type: resource.mimeType || '',
    size: resource.size || 0,
    source_page_url: pageUrl,
    resource_kind: resource.kind === 'hls' && resource.isLive === true ? 'live' : resource.kind,
    preferred_bandwidth: resource.preferredBandwidth || 0,
    preferred_height: resource.preferredHeight || 0,
    referer: sourceIdentity.referer || identity.referer,
    origin: sourceIdentity.origin || identity.origin,
    cookie: resourceCookie ?? await cookiesFor(resource.url, pageUrl, resource.tabId, resource.frameId ?? 0),
    user_agent: sourceIdentity.userAgent || identity.userAgent,
    request_headers: replayableRequestHeaders(chain?.requestHeaders || resource.requestHeaders),
    request_contexts: requestContexts,
    evidence: [...new Set((resource.evidence || []).map(value => String(value).trim()).filter(Boolean))].slice(0, 16),
    owner: String(resource.owner || '').slice(0, 160),
    confidence: boundedConfidence(resource.confidence),
    replay_context: replayMetadata(resource.replayContext),
    ...replay,
    extension_version: extension.version,
    extension_client_id: await browserClientId(),
    extension_browser: extension.browser,
  }
}

async function requestDownload(
  resource: MediaResource,
  chain?: RequestChain,
  options: ResourceRequestOptions = {},
) {
  if (validDownloadIdentity(resource.tabId)) await browser.storage.session.remove(`download-error:${resource.tabId}`)
  await hydrateHandoffTracker()
  const selectionKey = downloadSelectionKey(resource)
  const pending = !options.browserRequestId && [...trackedHandoffs.values()].find(item => item.tabId === resource.tabId
    && item.resourceId === resource.id && item.status === 'pending'
    && (!item.selectionKey || item.selectionKey === selectionKey))
  if (pending) {
    const previous = await native({ op: 'handoff_status', handoff_id: pending.id })
    if (!previous?.ok) throw new Error(previous?.error || '无法读取下载确认状态')
    updateTrackedHandoff(pending, previous.handoff || {})
    await persistHandoffTracker()
    if (pending.status === 'pending') return previous
  }
  const key = `${resource.tabId ?? -1}:${options.browserRequestId ? `request:${options.browserRequestId}` : resourceFingerprint(resource)}`
  const active = activeDownloadOffers.get(key)
  if (active) {
    const previous = await active
    if (previous?.ownership_uncertain) {
      if (activeDownloadOffers.get(key) === active) activeDownloadOffers.delete(key)
      return requestDownload(resource, chain, options)
    }
    if (!desktopAcceptedHandoff(previous)) return previous
    const status = await native({ op: 'handoff_status', handoff_id: previous.handoff.id })
    if (!status?.ok) throw new Error(status?.error || '无法读取下载确认状态')
    if (status.handoff?.status === 'pending') return previous
    if (activeDownloadOffers.get(key) === active) activeDownloadOffers.delete(key)
    else return requestDownload(resource, chain, options)
  }
  const request = (async () => {
    // Persist only a digest and the receipt key, before sending. Worker reloads
    // must query that same offer rather than generate a second download.
    const digest = await localDownloadIntentUrl(key)
    let created = false
    const records = await directOfferStore.update(DIRECT_OFFERS_STORAGE_KEY, current => {
      if (current.some(item => item.key === digest)) return current
      created = true
      return [...current, { key: digest, tabId: resource.tabId, clientRequestId: `download:${crypto.randomUUID()}` }]
    })
    const requestId = records.find(item => item.key === digest)!.clientRequestId
    let response: any
    if (created) {
      if (!options.desktopReady && !await desktopReadyForNewHandoff()) {
        return { ok: false, client_request_id: requestId, error: '主程序未连接，保留浏览器下载' }
      }
      try {
        response = await offer(resource, chain, options, requestId)
      } catch (error) {
        // offer catches transport errors itself; a throw here precedes send.
        await directOfferStore.update(DIRECT_OFFERS_STORAGE_KEY, current => current.filter(item => item.clientRequestId !== requestId))
        throw error
      }
    } else {
      try {
        response = await native({ op: 'handoff_status', client_request_id: requestId }, 2_500)
      } catch {}
      if (!response?.ok) return uncertainOffer(requestId)
      if (!response.handoff?.id) {
        // A read cannot exclude a delayed original offer. Only Core's atomic
        // admission can safely recover the same ID after journal-before-send.
        response = await offer(resource, chain, options, requestId, true)
      } else void trackHandoff(String(response.handoff.id), resource.id, resource.tabId, selectionKey).catch(() => undefined)
    }
    return { ...response, client_request_id: requestId }
  })()
    .then(response => {
      if (!response?.ok) {
        if (!response?.ownership_uncertain) {
          activeDownloadOffers.delete(key)
          void directOfferStore.update(DIRECT_OFFERS_STORAGE_KEY,
            current => current.filter(item => item.clientRequestId !== response.client_request_id)).catch(() => undefined)
        }
      } else void waitForHandoffResolution(String(response.handoff.id)).then(resolved => {
        if (handoffTerminalStatus(resolved.status)) {
          if (activeDownloadOffers.get(key) === request) activeDownloadOffers.delete(key)
          return directOfferStore.update(DIRECT_OFFERS_STORAGE_KEY,
            current => current.filter(item => item.clientRequestId !== response.client_request_id))
        }
      }).catch(() => undefined)
      return response
    }, error => {
      activeDownloadOffers.delete(key)
      throw error
    })
  activeDownloadOffers.set(key, request)
  return request
}

async function pushToTv(resource: MediaResource): Promise<{ ok: true; id: string }> {
  const response = await native({ op: 'media_push', kind: 'tvbox', resource: await resourcePayload(resource, undefined, { allowUnverified: true }) })
  const id = mediaPushRequestId(response, 'TVBox 推送')
  return { ok: true, id }
}

async function castToDevice(resource: MediaResource): Promise<{ ok: true; id: string }> {
  const response = await native({ op: 'media_push', kind: 'cast', resource: await resourcePayload(resource, undefined, { allowUnverified: true }) })
  const id = mediaPushRequestId(response, '投屏')
  return { ok: true, id }
}

function uncertainOffer(requestId: string) {
  return {
    ok: false, ownership_uncertain: true, client_request_id: requestId,
    error: '无法确认下载交接，已停止自动回退；请检查主程序状态，勿重复下载',
  }
}

let desktopReadinessCheck: Promise<boolean> | undefined

async function desktopReadyForNewHandoff(): Promise<boolean> {
  // 只供尚未发送的新交接使用，且必须在暂停原响应前检查；未知旧交接继续按原 ID 恢复。
  desktopReadinessCheck ??= native({ op: 'ping', interactive: true }, 1_500)
    .then(response => response?.ok === true, () => false)
    .finally(() => { desktopReadinessCheck = undefined })
  return desktopReadinessCheck
}

async function offer(resource: MediaResource, chain?: RequestChain, options: ResourceRequestOptions = {}, clientRequestId?: string, recovering = false) {
  // 只改写传输负载；原选择身份继续用于去重和弹窗恢复，避免同码率的不同线路串状态。
  const downloadResource = resource.manifestUrl && (resource.kind === 'hls' || resource.kind === 'dash')
    ? { ...resource, url: resource.manifestUrl } : resource
  const payload = {
    ...await resourcePayload(downloadResource, chain, options),
    // 每次下载拥有独立请求；同一 URL 的后续下载不能复用之前已接受或已拒绝的交接。
    // NativeBridge 重试复用同一负载，仍保留协议的幂等性。
    client_request_id: clientRequestId || `download:${crypto.randomUUID()}`,
  }
  let response: any
  try {
    // Older Hosts do not support recover_offer; their refusal must keep the
    // original ownership unresolved instead of replaying through old admission.
    response = await native({ op: recovering ? 'recover_offer' : 'offer', resource: payload })
  } catch (error) {
    response = { ok: false, transport_error: true, error: String(error) }
  }
  if (response?.transport_error || (recovering && !response?.ok)) {
    // Read the durable outcome; do not issue another offer or regenerate its
    // idempotency key after the transport retry budget has been exhausted.
    try {
      const recovered = await native({ op: 'handoff_status', client_request_id: payload.client_request_id }, 2_500)
      if (recovered?.ok && recovered.handoff?.id) response = recovered
    } catch {}
    if (!response?.ok) return uncertainOffer(payload.client_request_id)
  }
  const handoff = response?.handoff
  if (response?.ok && !handoff?.id) return uncertainOffer(payload.client_request_id)
  if (!response?.ok || !handoff?.id) return response
  if (handoff.presentation === 'failed' || handoff.presentation_ok === false) {
    return { ok: false, error: handoff.presentation_error || '桌面端未能打开下载确认窗口', handoff }
  }
  // Handoff creation is the acknowledgement. Presentation happens in the
  // desktop queue and must never hold the browser button at “发送中”.
  // 桌面已确认接收后，跟踪缓存写入失败不能把下载重新交还浏览器。
  void trackHandoff(String(handoff.id), resource.id, resource.tabId, downloadSelectionKey(resource))
    .catch(error => console.warn('HLS Downloader handoff tracking unavailable', error))
  return response
}

async function retainUncertainBrowserDownload(item: Browser.downloads.DownloadItem, response: any, tabId?: number): Promise<void> {
  await pausedFollowUpStore.remember({
    downloadId: item.id, handoffId: '', clientRequestId: response.client_request_id,
    phase: 'resolution', deadline: Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS,
  })
  schedulePausedFollowUpAccelerator(PAUSED_FOLLOW_UP_RECHECK_MS)
  await reportDownloadError(tabId ?? downloadIdentity(item).tabId, response.error)
}

async function reportDownloadError(tabId: number | undefined, reason: unknown): Promise<void> {
  if (!validDownloadIdentity(tabId)) return
  const error = reason instanceof Error ? reason.message : String(reason || '下载请求失败，请重试')
  // 受限页面无法注入提示时，下一次打开工具栏弹窗仍能看到失败原因。
  await browser.storage.session.set({ [`download-error:${tabId}`]: error }).catch(() => undefined)
  await browser.tabs.sendMessage(tabId, { type: 'download-error', error }, { frameId: 0 }).catch(() => undefined)
}

export async function refreshedDownload(downloadId: number, original: Browser.downloads.DownloadItem) {
  let determined = determinedDownloads.get(downloadId)
  if (!determined && filenameDeterminationEvent(import.meta.env.CHROME, browser.downloads as any)) {
    determined = await new Promise<Browser.downloads.DownloadItem | undefined>(resolve => {
      const timeout = setTimeout(() => {
        determinationWaiters.delete(downloadId)
        resolve(undefined)
      }, 450)
      determinationWaiters.set(downloadId, item => {
        clearTimeout(timeout)
        determinationWaiters.delete(downloadId)
        resolve(item)
      })
    })
  }
  const [current] = await browser.downloads.search({ id: downloadId })
  // The filename event precedes pause/cancel/completion. Only its filename is
  // a fallback; its old state and byte counts must not replace a fresh search.
  const refreshed = { ...original, ...(current || {}) }
  if (!refreshed.filename && determined?.filename) refreshed.filename = determined.filename
  return refreshed
}

async function removeBrowserDownload(item: Browser.downloads.DownloadItem, onlyIfStillParked = false): Promise<boolean> {
  let current: Browser.downloads.DownloadItem | undefined
  try {
    const [found] = await browser.downloads.search({ id: item.id })
    current = found
  } catch {
    return false
  }
  if (!current) return true
  // Receipt lookup can outlive a browser-side resume/cancel. Recheck at the
  // cleanup boundary so an old paused snapshot cannot override that choice.
  if (onlyIfStillParked && current.paused !== true && current.state !== 'complete') return true
  try {
    if (browserCleanupAction(current.state) === 'remove-file') {
      await browser.downloads.removeFile(item.id)
    } else {
      await browser.downloads.cancel(item.id)
      // 快速文件可能在 search 和 cancel 之间完成，不能先 erase 丢掉可清理的项目。
      for (let attempt = 0; attempt < 20; attempt++) {
        const [settled] = await browser.downloads.search({ id: item.id })
        if (!settled) return true
        if (settled.state === 'complete') {
          await browser.downloads.removeFile(item.id)
          break
        }
        if (settled.state === 'interrupted') break
        if (attempt === 19) return false
        await new Promise(resolve => setTimeout(resolve, 25))
      }
    }
    await browser.downloads.erase({ id: item.id })
  } catch {
    return false
  }
  return true
}

export async function completeBrowserTakeover(item: Browser.downloads.DownloadItem, handoffId: string): Promise<void> {
  // 回执已确认桌面接管；持久化清理责任与取消并行，不能让慢存储拖住浏览器副本。
  const followUp = rememberPausedHandoffFollowUp(item.id, handoffId)
  const removed = await removeBrowserDownload(item)
  await followUp
  if (removed) await pausedFollowUpStore.drop(handoffId)
}

function downloadRequestItem(
  item: Browser.downloads.DownloadItem,
  blobSource?: BlobSourceRecord,
): Browser.downloads.DownloadItem {
  if (!blobSource) return item
  return {
    ...item,
    url: blobSource.sourceUrl,
    finalUrl: blobSource.sourceUrl,
    referrer: blobSource.pageUrl || item.referrer,
  }
}

export async function pauseBrowserDownload(item: Browser.downloads.DownloadItem): Promise<boolean | null> {
  if (item.state === 'complete' || item.paused) return false
  if (item.state !== 'in_progress') return null
  try {
    if (import.meta.env.FIREFOX) {
      // Firefox 在网络通道尚未收到数据时暂停，会偶发变成无法恢复的 USER_CANCELED。
      // 先等原下载初始化；小文件已完成时保留它，待桌面成功后再清理。
      const deadline = Date.now() + 5_000
      while (true) {
        const [current] = await browser.downloads.search({ id: item.id })
        if (!current || current.state === 'interrupted') return null
        if (current.state === 'complete' || current.paused) return false
        if (current.bytesReceived > 0) break
        if (Date.now() >= deadline) return null
        await new Promise(resolve => setTimeout(resolve, 100))
      }
    }
    await browser.downloads.pause(item.id)
    const [current] = await browser.downloads.search({ id: item.id })
    if (!current || current.state === 'interrupted' && !current.canResume) return null
    return current.state !== 'complete'
  } catch {
    // 下载项事件是快照；策略读取期间小文件可能已完成，此时 pause 会拒绝。
    // 只有实际完成的文件可以继续交接，其他暂停失败仍不能启动第二份传输。
    try {
      const [current] = await browser.downloads.search({ id: item.id })
      if (current?.state === 'complete') return false
    } catch {}
    return null
  }
}

async function resumeBrowserDownload(item: Browser.downloads.DownloadItem, paused: boolean): Promise<boolean> {
  if (!paused) return true
  let current: Browser.downloads.DownloadItem | undefined
  try {
    const [found] = await browser.downloads.search({ id: item.id })
    current = found
  } catch {
    return false
  }
  if (!current) return true
  if (current.state === 'complete' || (current.state === 'in_progress' && current.paused === false)) return true
  if (current.state === 'interrupted' && current.error === 'USER_CANCELED' && !current.canResume) return true
  // Edge/Chrome may report a just-paused response as `interrupted` while it is
  // still resumable. Refusing that state left excluded downloads and failed or
  // rejected handoffs stuck forever at 0 B.
  if (!canResumeBrowserDownload(current.state)) return false
  try {
    await browser.downloads.resume(item.id)
    return true
  } catch {
    return false
  }
}

async function rememberClickIntent(intent: DownloadClickIntent): Promise<void> {
  await clickIntentStore.remember(intent)
  console.debug('HLS Downloader received an explicit click intent')
}

async function waitForClickIntent(
  url: string,
  finalUrl = '',
  referrer = '',
  chain?: RequestChain,
  attempts = 12,
  downloadAttributeOnly = false,
): Promise<DownloadClickIntent | undefined> {
  // The trusted pointerdown message normally precedes the network request.
  // Keep only a short MV3 wake-up grace period: a classified DownloadItem is
  // already strong evidence, so waiting seconds for a missing optional intent
  // merely delays the desktop prompt.
  const polls = Math.max(1, Math.min(12, Math.floor(attempts)))
  for (let attempt = 0; attempt < polls; attempt += 1) {
    const intent = await clickIntentStore.consume({
      url,
      finalUrl,
      referrer: referrer || chain?.pageUrl || '',
      chainUrls: chain?.urls,
      tabId: chain?.tabId,
      frameId: chain?.frameId,
      downloadAttributeOnly,
    })
    if (intent) return intent
    if (attempt + 1 < polls) await new Promise(resolve => setTimeout(resolve, 25))
  }
  return undefined
}

async function installContextMenus(attempt = 0): Promise<void> {
  try {
    await browser.contextMenus.removeAll()
    await Promise.all([
      browser.contextMenus.create({ id: 'hls-download-link', title: '使用 HLS Downloader 下载', contexts: ['link', 'video', 'audio'] }),
      browser.contextMenus.create({ id: 'hls-cast-link', title: '使用 HLS Downloader 投屏媒体链接', contexts: ['link', 'video', 'audio'], visible: !dynamicContextMenus.onShown }),
      browser.contextMenus.create({ id: 'hls-push-tvbox-link', title: '使用 HLS Downloader 推送媒体链接到 TVBox', contexts: ['link', 'video', 'audio'], visible: !dynamicContextMenus.onShown }),
      browser.contextMenus.create({ id: 'hls-download-selection', title: '批量发送选中的链接', contexts: ['selection'] }),
    ])
  } catch (error) {
    // Browser updates can briefly keep the old menu registry locked. Retry
    // once after the registry is released instead of silently losing all items.
    console.warn('HLS Downloader context menu install delayed', error)
    if (attempt < 3) setTimeout(() => { void installContextMenus(attempt + 1) }, 250 * (attempt + 1))
  }
}

function observedResponse(details: any, chain?: RequestChain) {
  if (!isMediaResponseStatus(details.statusCode) || !['GET', 'POST'].includes(String(details.method || 'GET').toUpperCase())) {
    return { disposition: '', resource: null }
  }
  const headers = details.responseHeaders || []
  const header = (name: string) => headers.find((item: any) => item.name?.toLowerCase() === name)?.value || ''
  const mimeType = header('content-type')
  const contentRange = header('content-range')
  const rangeTotal = Number(contentRange.match(/\/(\d+)$/)?.[1] || 0)
  const length = rangeTotal || Number(header('content-length') || 0)
  const disposition = header('content-disposition')
  const filename = responseFilename(disposition)
  // 附件导航和二进制导航已确定会进入下载器，不能套用被动嗅探对图片、JSON、无扩展名端点的排除规则。
  const downloadNavigation = ['main_frame', 'sub_frame'].includes(String(details.type || ''))
    && (/^\s*attachment\b/i.test(disposition) || mimeType.toLowerCase().includes('octet-stream'))
  const kind = disposition
    || mimeType.toLowerCase().includes('octet-stream')
    || isConcreteDownloadMime(mimeType)
    ? classifyDownload(details.url, mimeType, filename, disposition, downloadNavigation)
    : classifyResource(details.url, mimeType)
  if (!kind) return { disposition, resource: null }
  const resource = {
    url: details.url,
    kind,
    mimeType,
    size: length,
    filename,
    statusCode: details.statusCode,
    method: details.method,
    pageUrl: details.documentUrl || details.initiator || chain?.pageUrl || requestHeader(chain, 'referer') || '',
    tabId: details.tabId,
    frameId: details.frameId,
    requestHeaders: chain?.requestHeaders,
    responseIdentity: responseIdentity(header),
    evidence: [
      'response_headers',
      ...(disposition ? ['content_disposition'] : []),
      ...(chain?.requestId ? ['request_chain'] : []),
    ],
    owner: `tab:${Number(details.tabId ?? -1)}:frame:${Number(details.frameId ?? -1)}:${String(chain?.requestId || details.requestId || 'response')}`,
    confidence: disposition
      ? 0.99
      : /^video\//i.test(mimeType) || /^audio\//i.test(mimeType)
        ? 0.92
        : 0.84,
    replayContext: {
      method: String(details.method || 'GET').toUpperCase(),
      request_id: String(chain?.requestId || details.requestId || ''),
      final_url: String(chain?.finalUrl || details.url || ''),
    },
  }
  // saveResource/refreshTabBadge return the raw SessionListStore.update operation
  // (only the store's internal tail swallows errors), so a storage.session failure
  // would otherwise surface as an unhandled rejection in the service worker with
  // no caller able to log or recover from it.
  void saveResource(resource, details.tabId).catch(error => console.warn('HLS Downloader resource persistence failed', error))
  void inspectAdaptive(resource, details.tabId)
  void sendCapturedResource(details.tabId, resource)
  return { disposition, resource }
}

function trackedSize(chain: RequestChain | undefined): number {
  const contentRange = responseHeader(chain, 'content-range')
  const rangeTotal = Number(contentRange.match(/\/(\d+)$/)?.[1] || 0)
  return rangeTotal || Number(responseHeader(chain, 'content-length') || 0)
}

async function prepareEarlyBrowserTakeover(details: any, chain: RequestChain | undefined, observed: { disposition: string, resource: ObservedDownloadResource | null }) {
  if (!chain || !observed.resource) return null
  const requiresDownloadAttribute = !isEarlyDirectDownloadResponse(details, observed)
  if (requiresDownloadAttribute && (!import.meta.env.FIREFOX || !isEarlyDirectDownloadResponse(details, observed, true))) return null
  const observedResource = observed.resource
  try {
    const config = await settings()
    const resource = {
      ...observedResource,
      id: resourceId(observedResource.url),
      pageUrl: await topLevelPageUrl(Number(details.tabId), observedResource.pageUrl || chain.pageUrl),
      tabId: Number(details.tabId),
      frameId: Number(details.frameId),
      method: chain.method,
      requestHeaders: chain.requestHeaders,
      seenAt: Date.now(),
    } as MediaResource
    if (String(chain.method || '').toUpperCase() === 'POST') return null
    // Response headers are strong evidence that a navigation is a file, but
    // they are not evidence that the user asked for a download: autoplay,
    // hidden iframe preloads and direct MP4 navigations produce the same
    // headers. Bind the early offer to the same trusted click intent used by
    // the DownloadItem path so the browser is never pre-empted on a normal
    // page visit.
    const intent = await waitForClickIntent(
      resource.url,
      resource.url,
      resource.pageUrl || chain.pageUrl || '',
      chain,
      clickIntentPollsForKind(resource.kind),
      requiresDownloadAttribute,
    )
    // 同源 download 属性即使响应标记 inline 也会触发下载，普通预览导航仍交给浏览器。
    if (requiresDownloadAttribute && !intent) return null
    if (intent?.altBypass) return { resource, bypass: true }
    if (!intent && earlyTakeoverRequiresClick(resource.kind, observed.disposition)) return null
    if (!shouldTakeover({
      url: resource.url,
      sourcePageUrl: resource.pageUrl,
      size: resource.size,
      mimeType: resource.mimeType,
      filename: resource.filename,
      ...config,
      ...(intent || {}),
      explicitClick: Boolean(intent),
      strongEvidence: true,
    }) || (!intent?.ctrlForce && isHandoffSuppressed(config.suppressions, resource.pageUrl || '', resource.kind))) return null
    return { resource, intent }
  } catch (error) {
    console.debug('HLS Downloader early browser takeover unavailable', error)
    return null
  }
}

function rememberEarlyBrowserTakeover(details: any, chain: RequestChain | undefined, observed: { disposition: string, resource: ObservedDownloadResource | null }): EarlyBrowserTakeover | undefined {
  if (!chain || !observed.resource || !isEarlyDirectDownloadResponse(details, observed, import.meta.env.FIREFOX)) return
  const observedResource = observed.resource
  const requestId = String(chain.requestId || details.requestId || '')
  if (!requestId) return
  const existing = earlyBrowserTakeovers.get(requestId)
  if (existing) return existing
  const promise = prepareEarlyBrowserTakeover(details, chain, observed)
  const urls = [...new Set([observedResource.url, String(details.url || '')].filter(Boolean))]
  const entry = {
    requestId,
    startedAt: Date.now(),
    urls,
    tabId: Number(details.tabId),
    frameId: Number(details.frameId),
    promise,
  }
  earlyBrowserTakeovers.set(requestId, entry)
  void promise.finally(() => {
    setTimeout(() => {
      const current = earlyBrowserTakeovers.get(requestId)
      if (current === entry) earlyBrowserTakeovers.delete(requestId)
    }, 30_000)
  })
  // Keep the map bounded on pages that repeatedly navigate through downloads.
  if (earlyBrowserTakeovers.size > 128) {
    const oldest = [...earlyBrowserTakeovers.values()]
      .sort((left, right) => left.startedAt - right.startedAt)[0]
    if (oldest) earlyBrowserTakeovers.delete(oldest.requestId)
  }
  return entry
}

async function interceptFirefoxDownload(details: any, chain: RequestChain | undefined, observed: { disposition: string, resource: ObservedDownloadResource | null }) {
  const early = rememberEarlyBrowserTakeover(details, chain, observed)
  if (!early) return undefined
  const prepared = await early.promise
  if (!prepared || prepared.bypass) return undefined
  // 取消一个已经成功的 GET 后，无法保证 Core 还能重放一次性响应。
  // 文件响应继续到 onCreated，复用持久本地导入和失败保留原文件的路径。
  if (prepared.resource.kind === 'file' || prepared.resource.kind === 'media') return undefined
  if (!await desktopReadyForNewHandoff()) {
    early.promise = Promise.resolve({ resource: prepared.resource, bypass: true })
    return undefined
  }
  try {
    const response = await requestDownload(prepared.resource, chain, { desktopReady: true, browserRequestId: early.requestId, browserDownload: {
      url: chain?.initialUrl || details.url,
      finalUrl: chain?.finalUrl || details.url,
      tabId: details.tabId,
      frameId: details.frameId,
    } })
    if (response?.ownership_uncertain) {
      await reportDownloadError(prepared.resource.tabId, response.error)
      // Keep the original response. onCreated pauses its DownloadItem and
      // resolves the journaled offer by the same browser request identity.
      return undefined
    }
    // 阻塞原响应直到桌面确认接收，随后在 Firefox 建立 DownloadItem 之前取消。
    // 用户在桌面取消就是取消本次下载，不再启动一个浏览器副本。
    if (desktopAcceptedHandoff(response)) {
      earlyBrowserTakeovers.delete(early.requestId)
      return { cancel: true }
    }
  } catch (error) {
    console.warn('HLS Downloader response takeover unavailable', error)
  }
  // 本次已经选择浏览器回退，onCreated 不能再暂停并重复发送同一份请求。
  early.promise = Promise.resolve({ resource: prepared.resource, bypass: true })
  return undefined
}

const preparingFirefoxFileRequests = new Map<string, Promise<{ cancel: boolean } | undefined>>()

function interceptFirefoxFileRequest(details: any) {
  if (details.method !== 'GET' || !['main_frame', 'sub_frame'].includes(details.type)
    || !validDownloadIdentity(details.tabId)
    || (details.proxyInfo && details.proxyInfo.type !== 'direct')
    || !isDirectDownloadLink(details.url, '', false)) return undefined
  const chain = requestChains.observeRequest(details)
  // An authentication retry or redirect has already reached the server. Keep
  // that browser transfer and its original response on the existing path.
  if (chain.statusCode || chain.urls.length !== 1) return undefined
  const key = `${chain.tabId}:${chain.frameId}:${details.url.split('#', 1)[0]}`
  const active = preparingFirefoxFileRequests.get(key)
  if (active) return active
  // Cover preparation as well as the journaled offer. A repeated navigation
  // must not pass through while the first request is still reading cookies.
  const pending = prepareFirefoxFileRequest(details, chain).finally(() => {
    if (preparingFirefoxFileRequests.get(key) === pending) preparingFirefoxFileRequests.delete(key)
  })
  preparingFirefoxFileRequests.set(key, pending)
  return pending
}

async function prepareFirefoxFileRequest(details: any, chain: RequestChain) {
  const browserFallback = async () => {
    try {
      const key = `${chain.tabId}:${resourceFingerprint({ url: details.url, kind: 'file' })}`
      const active = activeDownloadOffers.get(key)
      let response = active ? await active : undefined
      if (!response || response.ownership_uncertain) {
        const digest = await localDownloadIntentUrl(key)
        const stored = await browser.storage.local.get(DIRECT_OFFERS_STORAGE_KEY)
        const records = stored[DIRECT_OFFERS_STORAGE_KEY] as Array<{ key: string, clientRequestId: string }> | undefined
        const requestId = response?.client_request_id || records?.find(item => item.key === digest)?.clientRequestId
        if (!requestId) return undefined
        try { response = await native({ op: 'handoff_status', client_request_id: requestId }, 2_500) } catch {}
        if (!response?.ok || !response.handoff?.id) response = uncertainOffer(requestId)
      }
      if (response.ownership_uncertain) await reportDownloadError(chain.tabId, response.error)
      return desktopAcceptedHandoff(response) || response.ownership_uncertain ? { cancel: true } : undefined
    } catch (error) {
      await reportDownloadError(chain.tabId, '无法确认已有下载交接，请检查主程序后重试')
      return { cancel: true }
    }
  }
  let resource: MediaResource
  let resourceCookies: ReplayCookie[]
  let intent: DownloadClickIntent | undefined
  try {
    const config = await settings()
    if (config.minimumBytes > 0) return browserFallback()
    const pageUrl = await topLevelPageUrl(chain.tabId, chain.pageUrl)
    intent = await waitForClickIntent(details.url, details.url, pageUrl, chain, 4)
    // Pointerdown/click deduplication can leave a second navigation without a
    // fresh intent. Existing ownership still applies, including after reload.
    if (!intent) return browserFallback()
    // Generic controls can match a later response; pre-request ownership needs
    // the exact link the user chose, before the server has identified a file.
    if (pageIdentity(intent.href) !== pageIdentity(details.url)) {
      await clickIntentStore.remember(intent)
      return browserFallback()
    }
    if (!shouldTakeover({ url: details.url, sourcePageUrl: pageUrl, ...config, ...intent, explicitClick: true })
      || (!intent.ctrlForce && isHandoffSuppressed(config.suppressions, pageUrl, 'file'))) {
      await clickIntentStore.remember(intent)
      return browserFallback()
    }
    // Cookie export remains permission-gated and uses the tab's cookie store.
    // If that snapshot cannot reproduce this request, preserve the browser's
    // response instead of starting a desktop request with different credentials.
    resourceCookies = await cookieRecordsFor(details.url, pageUrl, chain.tabId, chain.frameId, true)
    if (requestHeader(chain, 'cookie') !== cookieHeaderForUrl(resourceCookies, details.url)) {
      await clickIntentStore.remember(intent)
      return browserFallback()
    }
    resource = {
      id: resourceId(details.url), url: details.url, kind: 'file', pageUrl,
      tabId: chain.tabId, frameId: chain.frameId, method: 'GET',
      requestHeaders: chain.requestHeaders, seenAt: Date.now(),
      evidence: ['trusted_download_link', 'request_chain'], confidence: 0.99,
      replayContext: { method: 'GET', request_id: chain.requestId },
    }
  } catch (error) {
    if (intent) await clickIntentStore.remember(intent)
    console.debug('HLS Downloader pre-request takeover unavailable', error)
    return browserFallback()
  }
  try {
    // requestDownload resolves an existing journal before checking readiness
    // for a new offer. An offline desktop cannot release an uncertain offer.
    const response = await requestDownload(resource, chain, { resourceCookies })
    if (response?.ownership_uncertain) await reportDownloadError(resource.tabId, response.error)
    if (desktopAcceptedHandoff(response) || response?.ownership_uncertain) return { cancel: true }
  } catch (error) {
    await reportDownloadError(resource.tabId, '无法确认下载交接，请检查主程序后重试')
    return { cancel: true }
  }
  // A definite refusal/offline result leaves this download with the browser.
  // Its response and DownloadItem must not prompt for the same handoff again.
  await clickIntentStore.remember({ ...intent, altBypass: true, at: Date.now() })
  return undefined
}

export default defineBackground(() => {
  browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
    const previousPage = observedTabPages.get(tabId)
    const nextPage = changeInfo.url === undefined ? undefined : pageIdentity(changeInfo.url)
    if (nextPage !== undefined) observedTabPages.set(tabId, nextPage)
    // Reloads invalidate the document even at the same URL; fragment-only
    // updates preserve it. Unknown previous URLs are invalidated conservatively.
    if (changeInfo.status === 'loading' || nextPage !== undefined && nextPage !== previousPage) {
      requestChains.clearTab(tabId, true)
      adaptiveInspectionPages.get(tabId)?.abort()
      adaptiveInspectionPages.delete(tabId)
      inspectedAdaptive.releasePrefix(`${tabId}:`)
      void browser.action.setBadgeText({ tabId, text: '' }).catch(() => undefined)
      if (changeInfo.url) {
        void clearTabResources(tabId, pageResourceKey(tabId, changeInfo.url)).catch(() => undefined)
      }
    }
  })
  browser.tabs.onActivated.addListener(activeInfo => {
    void refreshTabBadge(activeInfo.tabId).catch(error => console.warn('HLS Downloader badge refresh failed', error))
  })
  browser.tabs.onRemoved.addListener(tabId => {
    observedTabPages.delete(tabId)
    void directOfferStore.update(DIRECT_OFFERS_STORAGE_KEY,
      current => current.filter(item => item.tabId !== tabId)).catch(() => undefined)
    void browser.storage.session.remove(`download-error:${tabId}`).catch(() => undefined)
    requestChains.clearTab(tabId)
    blobSources.clearTab(tabId)
    adaptiveInspectionPages.get(tabId)?.abort()
    adaptiveInspectionPages.delete(tabId)
    inspectedAdaptive.releasePrefix(`${tabId}:`)
    void clearTabResources(tabId).catch(() => undefined)
  })
  nativeBridge = new NativeBridge(
    () => browser.runtime.connectNative(HOST) as unknown as NativePortLike,
    30_000,
  )
  takeoverSettingsSync = new TakeoverSettingsSync(
    browser.storage.local,
    message => native(message),
  )
  const subtitleBridge = new NativeBridge(() => browser.runtime.connectNative(HOST) as unknown as NativePortLike, 20_000)
  const subtitleAudioBridge = new NativeBridge(() => browser.runtime.connectNative(HOST) as unknown as NativePortLike, 20_000)
  installSubtitleBackground(async request => (request.action === 'prepare_audio' ? subtitleAudioBridge : subtitleBridge)
    .request({ op: 'subtitle', request, client_id: await browserClientId() }),
    async (tabId, frameId, selection) => {
      const pageUrl = await topLevelPageUrl(tabId)
      const key = pageResourceKey(tabId, pageUrl)
      const values = await resourceSessionStore.update(key, raw => compactResources(pruneExpiredResources(raw), 40, true))
      let resource = values.find(value => value.id === selection.resourceId && resourceBelongsToFrame(value, frameId))
      if (!resource || !['hls', 'dash', 'media'].includes(resource.kind) || resource.isLive === true) {
        throw new Error('尚未确认当前播放器的点播音源，请继续播放以完成识别，或关闭提前翻译')
      }
      const tracks = resource.audioTracks || []
      const matches = tracks.filter(track => selection.audioLanguage && track.language === selection.audioLanguage
        || selection.audioLabel && track.label === selection.audioLabel)
      const track = matches.length === 1 ? matches[0] : tracks.length === 1 ? tracks[0] : undefined
      if (tracks.length > 1 && !track || selection.audioIndex > 0 && !track?.url) {
        throw new Error('无法确认当前备用音轨，请关闭提前翻译以采集播放器实际音轨')
      }
      if (track?.url) resource = { ...resource, url: track.url }
      // 已由实际加载的播放器唯一匹配；独立解码还会验证音频，不需要下载任务的请求完成门禁。
      return resourcePayload(resource, undefined, { allowUnverified: true })
    })
  void hydrateHandoffTracker().then(() => pollTrackedHandoffs()).catch(() => undefined)
  // A suspended worker may have left paused downloads waiting on a handoff;
  // storage.local survives suspension and browser restart, so finish that work right away.
  void continuePausedHandoffFollowups().catch(() => undefined)
  void continueLocalDownloadHandoffs().catch(() => undefined)
  void pingDesktop().catch(() => undefined)
  // One recurring alarm drives everything that must survive service-worker
  // suspension: the desktop heartbeat, handoff tracking, paused-download
  // follow-ups and the inactive-tab badge expiry. Only browser.alarms reliably
  // wake a suspended worker, so no deadline may ride on setTimeout alone.
  // Firefox clamps sub-minute periods and older Chromium builds reject them.
  void createRecurringAlarm(
    browser.alarms,
    WORKER_ALARM_NAME,
    WORKER_ALARM_PERIOD_MINUTES,
    import.meta.env.FIREFOX,
  )
  // Retired per-feature alarms from earlier builds would keep waking this
  // worker for listeners that no longer exist.
  void browser.alarms.clear('handoff-tracker').catch(() => undefined)
  void browser.alarms.clear('resource-badge-refresh').catch(() => undefined)
  browser.alarms.onAlarm.addListener(alarm => {
    if (alarm.name !== WORKER_ALARM_NAME) return
    requestChains.cleanup()
    blobSources.cleanup()
    void pingDesktop().catch(() => undefined)
    void pollTrackedHandoffs()
    void continuePausedHandoffFollowups().catch(() => undefined)
    void continueLocalDownloadHandoffs().catch(() => undefined)
    void refreshOpenTabBadgesWhenDue()
  })
  browser.runtime.onInstalled.addListener(() => {
    void installContextMenus()
  })
  // Firefox/Chromium can restore an existing service worker without emitting a
  // fresh install event. Ensure an upgrade/restart always reconstructs menus.
  void installContextMenus()

  const commandsApi = (browser as { commands?: { onCommand: { addListener: (listener: (command: string) => void) => void } } }).commands
  commandsApi?.onCommand.addListener(command => {
    if (!isSniffCurrentPageCommand(command)) return
    void browser.tabs.query({ active: true, currentWindow: true }).then(tabs => {
      const tab = tabs[0]
      if (tab?.id === undefined) return
      const dispatch = openMediaPanelCommandDispatch()
      void browser.tabs.sendMessage(tab.id, dispatch.message, dispatch.options).catch(() => undefined)
    })
  })

  ;(browser.webRequest.onSendHeaders.addListener as any)((details: any) => {
    requestChains.observeRequest(details)
  }, TRACKED_REQUEST_FILTER, requestHeaderExtraInfo(import.meta.env.CHROME))
  ;(browser.webRequest.onBeforeRequest.addListener as any)((details: any) => {
    requestChains.observeRequest(details)
  }, TRACKED_REQUEST_FILTER, ['requestBody'])
  if (import.meta.env.FIREFOX) {
    // Firefox can await a verdict with this request's headers before sending
    // HTTP data. Ordinary Chromium MV3 installations have no blocking API.
    ;(browser.webRequest.onBeforeSendHeaders.addListener as any)(interceptFirefoxFileRequest, {
      urls: ['http://*/*', 'https://*/*'], types: ['main_frame', 'sub_frame'],
    }, ['requestHeaders', 'blocking'])
  }
  browser.webRequest.onBeforeRedirect.addListener(details => {
    requestChains.observeRedirect(details as any)
  }, TRACKED_REQUEST_FILTER, ['responseHeaders'])

  // WXT 的共享类型沿用 Chromium 同步签名；Firefox 允许异步 BlockingResponse。
  ;(browser.webRequest.onHeadersReceived.addListener as any)((details: any) => {
    const chain = requestChains.observeResponse(details as any)
    const observed = observedResponse(details, chain)
    if (import.meta.env.FIREFOX && isEarlyDirectDownloadResponse(details, observed, true)) {
      return interceptFirefoxDownload(details, chain, observed)
    }
    // Chromium MV3 没有异步阻塞权限，在 onCreated 确认文件保全或媒体暂停路径。
    if (import.meta.env.CHROME) {
      rememberEarlyBrowserTakeover(details, chain, observed)
    }
    return undefined
  }, TRACKED_REQUEST_FILTER, import.meta.env.FIREFOX ? ['responseHeaders', 'blocking'] : ['responseHeaders'])
  browser.webRequest.onCompleted.addListener(details => {
    requestChains.finish(details.requestId, details.timeStamp || Date.now())
  }, TRACKED_REQUEST_FILTER)
  browser.webRequest.onErrorOccurred.addListener(details => {
    requestChains.fail(details.requestId)
  }, TRACKED_REQUEST_FILTER)

  filenameDeterminationEvent(import.meta.env.CHROME, browser.downloads as any)?.addListener((item: any, suggest: any) => {
    determinedDownloads.set(item.id, item)
    setTimeout(() => determinedDownloads.delete(item.id), 30_000)
    determinationWaiters.get(item.id)?.(item)
    suggest()
  })

  browser.downloads.onCreated.addListener(async item => {
    if (!item.url) return
    const blobSource = item.url.startsWith('blob:') ? blobSources.find(item.url) : undefined
    const originalRequest = downloadRequestItem(item, blobSource)
    const creatingExtension = String((item as any).byExtensionId || '')
    if (creatingExtension && creatingExtension !== browser.runtime.id) return
    if (item.url.startsWith('blob:') || item.url.startsWith('data:')) {
      const config = await settings()
      if (!config.enabled) return
      const sourceKey = await localDownloadIntentUrl(item.url)
      const intent = await waitForClickIntent(sourceKey, undefined, item.referrer || '', undefined, 12)
      if (intent?.altBypass) return
      const blobOrigin = item.url.startsWith('blob:') ? new URL(item.url).origin : ''
      const pageUrl = await topLevelPageUrl(intent?.tabId ?? blobSource?.tabId ?? downloadIdentity(item).tabId ?? -1,
        intent?.pageUrl || blobSource?.pageUrl || item.referrer || (blobOrigin !== 'null' ? blobOrigin : ''))
      if (!pageUrl && config.excludedHosts.length) return
      await queueLocalBrowserDownload({ downloadId: item.id, requestId: `local-file:${crypto.randomUUID()}`, sourceKey, pageUrl, intent })
      return
    }
    console.debug('HLS Downloader observed a browser download candidate')
    let paused = false
    // Once a handoff may own the transfer, finally must leave the durable
    // paused browser fallback alone. This also covers transient status loss;
    // it does not mean Core acceptance was observed.
    let browserOwnedByHandoff = false
    try {
      const config = await settings()
      if (!config.enabled) return
      let provisionalChain = requestChains.find(originalRequest, Date.now(), blobSource?.tabId ?? downloadIdentity(item).tabId, false, 'exact')
      // 优先使用浏览器来源或唯一请求链。Chrome 缺少来源时，也不能按时间猜标签页。
      const earlyTakeover = (provisionalChain
        ? earlyBrowserTakeovers.get(provisionalChain.requestId)
        : undefined)
        ?? findEarlyBrowserTakeoverByUrl(
          [(item as any).finalUrl, item.url, originalRequest.url],
          downloadIdentity(item),
          earlyBrowserTakeovers.values(),
        )
      // 原请求可跨过 Firefox 后台重载。按浏览器 requestId 恢复已有接管，
      // 不能在用户已拒绝后仅凭相同 URL 再弹一次确认。
      await hydrateHandoffTracker()
      const originalChain = provisionalChain
      const restored = !earlyTakeover && originalChain
        ? [...trackedHandoffs.values()].find(tracked => tracked.browserRequestId === originalChain.requestId
          && tracked.tabId === originalChain.tabId)
        : undefined
      if (earlyTakeover || restored) {
        const earlyResult = await earlyTakeover?.promise
        if (earlyTakeover) earlyBrowserTakeovers.delete(earlyTakeover.requestId)
        if (earlyResult?.bypass) return
        if (earlyResult || restored) {
          if (!restored && earlyResult
            && await preserveBrowserDownload(item, earlyResult.resource, earlyResult.intent)) return
          const pauseResult = await pauseBrowserDownload(item)
          if (pauseResult === null) return
          paused = pauseResult
          const response = earlyResult
            ? await requestDownload(earlyResult.resource, provisionalChain, {
              browserRequestId: earlyTakeover?.requestId, browserDownload: originalRequest,
            }) : undefined
          if (response?.ownership_uncertain) {
            browserOwnedByHandoff = true
            await retainUncertainBrowserDownload(item, response, earlyResult?.resource.tabId)
            return
          }
          if (!restored && !desktopAcceptedHandoff(response)) return
          const handoffId = String(response?.handoff?.id || restored?.id || '')
          // 持久回执即为交接边界；后续取消/重试属于桌面，不再启动浏览器副本。
          browserOwnedByHandoff = true
          await completeBrowserTakeover(item, handoffId)
          if (earlyTakeover && earlyResult && response?.handoff?.id) {
            const tracked = await trackHandoff(handoffId, earlyResult.resource.id, earlyResult.resource.tabId)
            tracked.browserRequestId = earlyTakeover.requestId
            await persistHandoffTracker()
          }
          return
        }
      }
      // 响应头已提供附件名时，无需等浏览器再次确定文件名；无名下载仍读取最终元数据。
      const classifyBrowserItem = (
        browserItem: Browser.downloads.DownloadItem,
        chain?: RequestChain,
      ) => {
        const actual = downloadRequestItem(browserItem, blobSource)
        const url = chain?.finalUrl || actual.finalUrl || actual.url
        const contentDisposition = responseHeader(chain, 'content-disposition')
        const responseName = responseFilename(contentDisposition)
        const filename = responseName || actual.filename.split(/[\\/]/).pop() || ''
        const mimeType = actual.mime || responseHeader(chain, 'content-type')
        return {
          actual,
          url,
          contentDisposition,
          filename,
          mimeType,
          kind: classifyDownload(url, mimeType, filename, contentDisposition, true),
        }
      }
      let classified = classifyBrowserItem(item, provisionalChain)
      let actualBrowser = item
      if (!item.filename && !classified.filename) {
        actualBrowser = await refreshedDownload(item.id, item)
        if (!canContinueTakeover(actualBrowser.state, paused)) return
        provisionalChain = requestChains.find(downloadRequestItem(actualBrowser, blobSource), Date.now(), blobSource?.tabId ?? downloadIdentity(actualBrowser).tabId, false, 'exact')
        classified = classifyBrowserItem(actualBrowser, provisionalChain)
      } else {
        const [current] = await browser.downloads.search({ id: item.id }).catch(() => [])
        actualBrowser = { ...item, ...(current || {}) }
        if (!canContinueTakeover(actualBrowser.state, paused)) return
      }
      if (!classified.kind) return
      let intent = await waitForClickIntent(
        classified.actual.url,
        classified.actual.finalUrl,
        classified.actual.referrer || provisionalChain?.pageUrl || '',
        provisionalChain,
        clickIntentPollsForKind(classified.kind),
      )
      // 新标签页下载的点击属于父标签页，不能覆盖已确认的请求来源。
      let chain = provisionalChain ?? (intent?.tabId === undefined
        ? undefined
        : requestChains.find(classified.actual, Date.now(), downloadIdentity(actualBrowser).tabId ?? intent.tabId, false, 'exact'))
      if (chain && chain !== provisionalChain) {
        classified = classifyBrowserItem(actualBrowser, chain)
        if (!classified.kind) return
      }
      const { actual, url, filename, mimeType, kind } = classified
      const size = (actual.fileSize && actual.fileSize > 0 ? actual.fileSize : 0)
        || (actual.totalBytes && actual.totalBytes > 0 ? actual.totalBytes : 0)
        || trackedSize(chain)
      const pageUrl = actual.referrer || chain?.pageUrl || requestHeader(chain, 'referer')
      const sourcePageUrl = await topLevelPageUrl(chain?.tabId ?? blobSource?.tabId ?? -1, pageUrl)
      const resource: MediaResource = {
        id: resourceId(url), url, kind, mimeType, size, title: filename, filename,
        pageUrl: sourcePageUrl,
        tabId: chain?.tabId ?? blobSource?.tabId,
        frameId: chain?.frameId ?? blobSource?.frameId,
        method: chain?.method,
        requestHeaders: chain?.requestHeaders,
        seenAt: Date.now(),
      }
      if (!shouldTakeover({
        url: resource.url,
        sourcePageUrl: resource.pageUrl,
        size: resource.size,
        mimeType,
        filename,
        ...config,
        ...(intent || {}),
        explicitClick: Boolean(intent),
        strongEvidence: true,
      }) || (!intent?.ctrlForce && isHandoffSuppressed(config.suppressions, resource.pageUrl || '', resource.kind))) {
        return
      }
      if (await preserveBrowserDownload(actualBrowser, resource, intent)) return
      // DownloadItem 可能先于请求链到达。使用与 offer 相同的解析流程确认方法后
      // 才暂停，且把同一条链传入 offer，避免晚到的 POST 被当成 GET 路径重放。
      chain = (await resourceRequest(resource, chain, { browserDownload: classified.actual })).chain
      if (!chain) return
      if (chain.method.toUpperCase() === 'POST') {
        // 复用本地导入，复制成功前保留浏览器源文件。
        await queueLocalBrowserDownload({
          downloadId: item.id, requestId: `local-file:${crypto.randomUUID()}`,
          sourceKey: await localDownloadIntentUrl(item.url), pageUrl: resource.pageUrl || '', intent,
          importUrl: 'data:application/octet-stream,',
        })
        return
      }
      if (!await desktopReadyForNewHandoff()) return
      const pauseResult = await pauseBrowserDownload(actualBrowser)
      if (pauseResult === null) return
      paused = pauseResult
      console.debug('HLS Downloader offering a verified browser download')
      const response = await requestDownload(resource, chain, {
        desktopReady: true, browserRequestId: chain.requestId, browserDownload: classified.actual,
      })
      if (response?.ownership_uncertain) {
        browserOwnedByHandoff = true
        await retainUncertainBrowserDownload(actualBrowser, response, resource.tabId)
        return
      }
      if (!desktopAcceptedHandoff(response)) throw new Error(response?.error || 'desktop rejected')
      const handoffId = String(response.handoff.id)
      browserOwnedByHandoff = true
      await completeBrowserTakeover(actualBrowser, handoffId)
    } catch (error) {
      console.warn('HLS Downloader browser handoff failed', error)
    } finally {
      determinedDownloads.delete(item.id)
      determinationWaiters.delete(item.id)
      await resumeBrowserDownload(item, paused && !browserOwnedByHandoff)
    }
  })

  browser.downloads.onChanged.addListener(change => {
    if (change.state?.current === 'complete' || change.state?.current === 'interrupted') {
      void continueLocalDownloadHandoffs().catch(() => undefined)
    }
  })

  browser.contextMenus.onClicked.addListener((info, tab) => {
    const frameId = Number((info as any).frameId)
    if (info.menuItemId === 'hls-download-selection') {
      if (tab?.id !== undefined) {
        void browser.tabs.sendMessage(tab.id, { type: 'collect-selection' },
          Number.isInteger(frameId) && frameId >= 0 ? { frameId } : undefined)
          .catch(error => reportDownloadError(tab.id!, `无法读取选中的链接：${String(error)}`))
      }
      return
    }
    const capabilities = contextMenuCapabilities(info)
    const url = capabilities.url
    if (!url) return
    const mediaAction = info.menuItemId === 'hls-cast-link' || info.menuItemId === 'hls-push-tvbox-link'
    if (mediaAction && !capabilities.media) return
    const kind = classifyResource(url)
      || ((info.mediaType === 'video' || info.mediaType === 'audio') ? classifyPlaybackSource(url) : null)
      || (info.menuItemId === 'hls-download-link' && /^(?:https?|magnet):/i.test(url) ? 'file' : null)
    if (!kind || /^blob:/i.test(url)) {
      // MSE players expose only a blob: URL in the browser context menu. It is
      // not a downloadable origin; open the correlated player panel instead
      // of mislabelling it as a file task.
      if (tab?.id !== undefined) {
        void browser.tabs.sendMessage(
          tab.id,
          { type: 'open-media-panel' },
          Number.isInteger(frameId) && frameId >= 0 ? { frameId } : undefined,
        ).catch(() => undefined)
      }
      return
    }
    const resource = {
      id: resourceId(url), url, kind, pageUrl: info.frameUrl || tab?.url, title: tab?.title, tabId: tab?.id,
      frameId: Number.isInteger(frameId) && frameId >= 0 ? frameId : 0,
      seenAt: Date.now(), evidence: ['context_menu'], owner: 'context-menu', confidence: 0.99,
      replayContext: { method: 'GET', page_url: String(tab?.url || '') },
    }
    if (info.menuItemId === 'hls-cast-link') {
      void castToDevice(resource).catch(error => console.warn('HLS Downloader context cast failed', error))
      return
    }
    if (info.menuItemId === 'hls-push-tvbox-link') {
      void pushToTv(resource).catch(error => console.warn('HLS Downloader context TVBox push failed', error))
      return
    }
    // Choosing the extension's context-menu command is already an explicit
    // confirmation, just like the popup and in-player Download buttons.
    void requestDownload(resource, undefined, { allowUnverified: true })
      .then(response => {
        if (!response?.ok) return reportDownloadError(tab?.id, response?.error || '桌面端未接受下载请求')
      })
      .catch(error => reportDownloadError(tab?.id, error))
  })

  dynamicContextMenus.onShown?.addListener(info => {
    const capabilities = contextMenuCapabilities(info)
    void Promise.all([
      browser.contextMenus.update('hls-download-link', { enabled: capabilities.download }),
      browser.contextMenus.update('hls-cast-link', { visible: capabilities.media, enabled: capabilities.media }),
      browser.contextMenus.update('hls-push-tvbox-link', { visible: capabilities.media, enabled: capabilities.media }),
    ]).then(() => dynamicContextMenus.refresh?.()).catch(error => {
      console.warn('HLS Downloader context menu refresh failed', error)
    })
  })

  browser.runtime.onMessage.addListener((message, sender, sendResponse) => {
    if (message?.type === 'intercept-download-link') {
      const href = String(message.href || '')
      const pageUrl = String(sender.url || sender.tab?.url || '')
      const tabId = sender.tab?.id
      void (async () => {
        const config = await settings()
        if (!validDownloadIdentity(tabId) || config.minimumBytes > 0
          || !isDirectDownloadLink(href, pageUrl, Boolean(message.downloadAttribute))
          || !shouldTakeover({ url: href, sourcePageUrl: pageUrl, ...config, explicitClick: true })
          || isHandoffSuppressed(config.suppressions, pageUrl, 'file')) {
          return { ok: false, browserFallback: true }
        }
        await clickIntentStore.consume({ url: href, referrer: pageUrl, tabId })
        const resource: MediaResource = {
          id: resourceId(href), url: href, kind: 'file', pageUrl, tabId,
          frameId: sender.frameId ?? 0, filename: String(message.filename || ''),
          method: 'GET', seenAt: Date.now(), evidence: ['trusted_download_link'], confidence: 0.99,
          replayContext: { method: 'GET', page_url: pageUrl },
        }
        try {
          const response = await requestDownload(resource, undefined, { allowUnverified: true })
          if (response?.ownership_uncertain) await reportDownloadError(tabId, response.error)
          return { ...response, browserFallback: !desktopAcceptedHandoff(response) && !response?.ownership_uncertain }
        } catch (error) {
          // offer 发送后失联可能已持久接收，不能自动重新导航产生第二份下载。
          await reportDownloadError(tabId, '无法确认下载交接，请检查主程序后重试')
          return { ok: false, browserFallback: false, error: String(error) }
        }
      })().then(sendResponse, error => sendResponse({ ok: false, browserFallback: true, error: String(error) }))
      return true
    }
    if (message?.type === 'blob-source') {
      const tabId = Number(sender.tab?.id ?? -1)
      if (tabId >= 0) {
        blobSources.remember({
          blobUrl: String(message.blobUrl || ''),
          sourceUrl: String(message.sourceUrl || ''),
          tabId,
          frameId: Number(sender.frameId ?? -1),
          pageUrl: String(message.pageUrl || sender.url || sender.tab?.url || ''),
        })
      }
      return
    }
    if (message?.type === 'click-intent') {
      // Keep the MV3 worker alive until the intent is durable. Without an
      // asynchronous response, Chrome may suspend the worker between the
      // click and downloads.onCreated, which makes normal file downloads look
      // unrelated and leaves them in the browser.
      void rememberClickIntent({
        href: String(message.href || ''),
        pageUrl: String(message.pageUrl || sender.tab?.url || ''),
        altBypass: Boolean(message.altBypass),
        ctrlForce: Boolean(message.ctrlForce),
        generic: Boolean(message.generic),
        tabId: sender.tab?.id,
        frameId: sender.frameId,
        opensNewTab: Boolean(message.opensNewTab),
        controlHint: Boolean(message.controlHint),
        downloadAttribute: Boolean(message.downloadAttribute),
        at: Date.now(),
      })
        .then(() => sendResponse({ ok: true }))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'resource') {
      const observedIdentity = responseIdentity(name => message.resource.responseIdentity?.[name])
      const observedChain = Object.keys(observedIdentity).length ? requestChains.find({
        url: message.resource.url, referrer: sender.url, frameId: sender.frameId,
        responseIdentity: observedIdentity,
      }, Date.now(), sender.tab?.id, true) : undefined
      const resource = {
        ...message.resource,
        // Observations belong to the sending frame, never a payload-selected one.
        pageUrl: sender.url || sender.tab?.url || '',
        tabId: sender.tab?.id ?? -1,
        frameId: sender.frameId ?? -1,
        responseIdentity: observedIdentity,
        ...(Object.keys(observedIdentity).length ? {
          requestHeaders: observedChain?.requestHeaders || {}, statusCode: observedChain?.statusCode, method: observedChain?.method,
          replayContext: { ...(message.resource.replayContext || {}), request_id: observedChain?.requestId || '' },
        } : {}),
      }
      void saveResource(resource, sender.tab?.id ?? -1)
        .then(() => sendResponse({ ok: true }))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      void inspectAdaptive(resource, sender.tab?.id ?? -1)
      return true
    }
    if (message?.type === 'download-now') {
      const resource = {
        ...message.resource,
        pageUrl: message.resource.pageUrl || sender.url || sender.tab?.url || '',
        tabId: message.resource.tabId ?? sender.tab?.id,
        frameId: message.resource.frameId ?? sender.frameId,
      }
      // 插件加载前已播放或命中缓存的媒体可能没有响应链；用户主动点击仍应交给 Core。
      void requestDownload(resource, undefined, { allowUnverified: true })
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'handoff-status') {
      void native({ op: 'handoff_status', handoff_id: String(message.handoffId || '') }, 2_500)
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'pending-downloads') {
      const tabId = Number(message.tabId ?? sender.tab?.id ?? -1)
      void hydrateHandoffTracker().then(() => {
        sendResponse([...trackedHandoffs.values()].filter(item => item.tabId === tabId && item.status === 'pending'))
      }).catch(() => sendResponse([]))
      return true
    }
    if (message?.type === 'local-download-progress') {
      // Only the extension popup may query this page-scoped, non-secret projection.
      if (sender.tab || sender.url && !sender.url.startsWith(browser.runtime.getURL(''))) return false
      const tabId = Number(message.tabId ?? -1)
      void (async () => {
        await hydrateLocalDownloads()
        const tab = tabId >= 0 ? await browser.tabs.get(tabId).catch(() => undefined) : undefined
        const records = [...localDownloadHandoffs.values()].filter(record => record.tabId === tabId
          || (record.tabId == null || record.tabId < 0) && record.pageUrl && record.pageUrl === tab?.url)
        return Promise.all(records.map(async record => {
          const item = await findLocalBrowserDownload(record).catch(() => undefined)
          return {
            id: record.requestId,
            filename: item?.filename?.split(/[\\/]/).pop() || '浏览器下载',
            bytesReceived: Math.max(0, item?.bytesReceived ?? 0),
            totalBytes: Math.max(0, item?.totalBytes ?? 0),
            state: !item ? 'recovering' : item.state === 'interrupted' ? 'interrupted'
              : item.state === 'complete' ? 'importing' : item.paused ? 'paused' : 'receiving',
          }
        }))
      })().then(sendResponse).catch(() => sendResponse([]))
      return true
    }
    if (message?.type === 'list') {
      const tabId = Number(message.tabId ?? sender.tab?.id ?? -1)
      // Content scripts run in every frame, while the session cache is kept
      // page-scoped so the popup can aggregate all players.  A frame overlay
      // must receive only its own observations; otherwise two same-page MSE
      // players (especially in cross-origin iframes) can exchange manifests
      // and show a download button for the wrong video.  Extension pages such
      // as the popup do not have an http(s) sender URL, so they keep the
      // aggregate view. The top-frame tab panel explicitly requests it too.
      const pageSender = /^https?:\/\//i.test(String(sender.url || ''))
      const senderFrameId = pageSender && Number.isInteger(Number(sender.frameId))
        ? Number(sender.frameId)
        : -1
      const aggregateTab = shouldAggregateTabMedia(message.scope, senderFrameId)
      void topLevelPageUrl(tabId, String(message.pageUrl || ''))
        .then(pageUrl => pageResourceKey(tabId, pageUrl))
        .then(key => resourceSessionStore.update(key, raw => compactResources(pruneExpiredResources(raw), 40, true)).then(async cleaned => {
          // Never write the frame-filtered view back to the page cache: doing
          // so would erase sibling iframe resources just because one frame
          // opened its panel first.
          const scoped = senderFrameId >= 0 && !aggregateTab
            ? cleaned.filter(resource => resourceBelongsToFrame(resource, senderFrameId))
            : cleaned
          const visible = senderFrameId >= 0 && !aggregateTab ? compactResources(scoped, 40, true) : compactResources(cleaned, 40)
          // The toolbar badge is tab-wide. An iframe asking for its scoped
          // overlay list must never overwrite that badge with only its own
          // candidates (or clear resources found by a sibling/top frame).
          await setResourceBadge(tabId, cleaned)
          sendResponse(visible)
        }))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'ping') {
      void pingDesktop()
        .then(response => sendResponse(response))
        .catch(error => sendResponse({
          ok: false,
          reconnecting: lastDesktopPingAt > 0 && Date.now() - lastDesktopPingAt < 90_000,
          error: String(error),
        }))
      return true
    }
    if (message?.type === 'set-takeover-settings') {
      const update = {
        ...(typeof message.enabled === 'boolean' ? { enabled: message.enabled } : {}),
        ...(Number.isFinite(Number(message.minimumBytes)) ? { minimumBytes: Number(message.minimumBytes) } : {}),
      }
      void (takeoverSettingsSync
        ? takeoverSettingsSync.queue(update)
        : Promise.reject(new Error('接管设置同步尚未初始化')))
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'push-to-tv') {
      const resource = { ...message.resource }
      void pushToTv(resource)
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'cast-to-device') {
      const resource = { ...message.resource }
      void castToDevice(resource)
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'media-push-status') {
      void native({ op: 'media_push_status', request_id: String(message.requestId || '') })
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'activate') {
      void native({ op: 'activate' })
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
  })
})
