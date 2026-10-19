import { browser } from 'wxt/browser'
import { downloadSelectionKey, localDownloadIntentUrl } from '../lib/resources'
import { mediaPushRequestId } from '../lib/mediaPush'
import { isSniffCurrentPageCommand, openMediaPanelCommandDispatch, shouldAggregateTabMedia } from '../lib/sniffCommand'
import { NativeBridge, type NativePortLike } from '../lib/nativeBridge'
import { boundedConfidence, canonicalMediaUrl, capturedRequestIdentity, classifyDownload, classifyPlaybackSource, classifyResource, compactResources, isConcreteDownloadMime, isShortLivedMediaSignatureUsable, mergeResources, normalizeHost, pageResourceKey, pruneExpiredResources, replayableRequestHeaders, resourceBelongsToFrame, resourceFingerprint, resourceId, resourceRequestIdentity, shouldTakeover, suggestedResourceFilename, usesShortLivedMediaSignature, type DownloadClickIntent, type MediaResource } from '../lib/resources'
import { RequestChainStore, replayablePostRequest, requestHeader, responseHeader, type RequestChain } from '../lib/requestChain'
import { browserCleanupAction, canContinueTakeover, canResumeBrowserDownload, desktopAcceptedHandoff, desktopTaskReadiness, localFileTaskReadiness, handoffTerminalStatus, type BrowserHandoffPayload, type DesktopTaskReadiness } from '../lib/takeover'
import {
  PAUSED_FOLLOW_UP_RECHECK_MS,
  PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY,
  PAUSED_HANDOFF_RESOLUTION_MS,
  PausedHandoffFollowUpStore,
  type PausedHandoffFollowUp,
  stepPausedHandoffFollowUp,
} from '../lib/pausedHandoffFollowups'
import { HANDOFF_SUPPRESSION_STORAGE_KEY, isHandoffSuppressed, normalizeHandoffSuppressions } from '../lib/handoffSuppression'
import { createRecurringAlarm, filenameDeterminationEvent, requestHeaderExtraInfo } from '../lib/browserCapabilities'
import { inspectHlsResource } from '../lib/hlsInspection'
import { inspectDashResource } from '../lib/dashInspection'
import { contentDispositionFilename } from '../lib/contentDisposition'
import { InspectionCache } from '../lib/inspectionCache'
import { cookieHeaderForUrl, cookieLookupUrl, cookieMatchesUrl, cookiePartitionKeyForSrcdoc, cookiePartitionSite, cookiePermissionAllows, isCookieHostPermissionError, isSrcdocCookieHostPermissionError, normalizeCookiePermissionHosts, replayCookieIdentity, type ReplayCookie, type SrcdocFrameContext } from '../lib/browserCookies'
import { detectBrowserFamily, stableBrowserClientId } from '../lib/browserClient'
import { isEarlyDirectDownloadResponse, type ObservedDownloadResource } from '../lib/directResponse'
import { clickIntentPollsForKind, earlyTakeoverRequiresClick } from '../lib/fileTakeover'
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
  downloadId: number
  requestId: string
  sourceKey: string
  pageUrl: string
  importUrl?: string
  handoffId?: string
  intent?: DownloadClickIntent
}
const localDownloadHandoffs = new Map<number, LocalDownloadHandoff>()
let localDownloadsHydration: Promise<void> | undefined
let localDownloadsPass = false
let localDownloadsTimer: ReturnType<typeof setTimeout> | undefined
let localDownloadsWrite: Promise<unknown> = Promise.resolve()

async function hydrateLocalDownloads(): Promise<void> {
  localDownloadsHydration ??= browser.storage.local.get(LOCAL_DOWNLOADS_KEY).then(data => {
    const stored = data[LOCAL_DOWNLOADS_KEY]
    for (const record of Array.isArray(stored) ? stored as LocalDownloadHandoff[] : []) {
      if (Number.isInteger(record.downloadId) && typeof record.requestId === 'string') {
        localDownloadHandoffs.set(record.downloadId, record)
      }
    }
  })
  await localDownloadsHydration
}

async function persistLocalDownloads(): Promise<void> {
  const records = [...localDownloadHandoffs.values()]
  localDownloadsWrite = localDownloadsWrite.catch(() => undefined)
    .then(() => browser.storage.local.set({ [LOCAL_DOWNLOADS_KEY]: records }))
  await localDownloadsWrite
}

async function continueLocalDownloadHandoffs(): Promise<void> {
  if (localDownloadsPass) return
  localDownloadsPass = true
  try {
    await hydrateLocalDownloads()
    for (const record of localDownloadHandoffs.values()) {
      try {
        const [item] = await browser.downloads.search({ id: record.downloadId })
        if (!item || (item.state === 'interrupted' && item.error === 'USER_CANCELED')) {
          localDownloadHandoffs.delete(record.downloadId)
          await persistLocalDownloads()
          continue
        }
        if (item.state !== 'complete' || !item.filename) continue
        if (await localDownloadIntentUrl(item.url) !== record.sourceKey) {
          localDownloadHandoffs.delete(record.downloadId)
          await persistLocalDownloads()
          continue
        }
        if (!record.handoffId) {
          const config = await settings()
          const size = Math.max(0, item.fileSize >= 0 ? item.fileSize : item.bytesReceived)
          if (!shouldTakeover({ url: item.url, sourcePageUrl: record.pageUrl, size,
            ...config, ...record.intent, strongEvidence: true, localBrowserDownload: true })
            || (!record.intent?.ctrlForce && isHandoffSuppressed(config.suppressions, record.pageUrl, 'file'))) {
            localDownloadHandoffs.delete(record.downloadId)
            await persistLocalDownloads()
            continue
          }
          // data URL 内容可能远超 Native Messaging 限制；文件字节只从已完成的本地文件复制。
          const url = record.importUrl || (item.url.startsWith('data:') ? 'data:application/octet-stream,' : item.url)
          const response = await native({ op: 'offer_local_file', resource: {
            url, local_source_path: item.filename, filename: item.filename.split(/[\\/]/).pop(),
            mime_type: item.mime, size, resource_kind: 'file', source_page_url: record.pageUrl,
            client_request_id: record.requestId, evidence: ['browser_completed_local_file'],
          } })
          if (!desktopAcceptedHandoff(response)) {
            localDownloadHandoffs.delete(record.downloadId)
            await persistLocalDownloads()
            continue
          }
          record.handoffId = String(response.handoff.id)
          await persistLocalDownloads()
          await trackHandoff(record.handoffId)
        }
        const response = await native({ op: 'handoff_status', handoff_id: record.handoffId }, 2_500)
        const handoff = (response?.handoff || response || {}) as BrowserHandoffPayload
        const readiness = localFileTaskReadiness(handoff)
        if (readiness === 'waiting') continue
        await rememberHandoffSuppression((handoff as any).suppression)
        if (readiness === 'safe-to-remove' && !await removeBrowserDownload(item)) continue
        localDownloadHandoffs.delete(record.downloadId)
        await persistLocalDownloads()
      } catch (error) {
        // 状态不明时保留源文件，并以持久 request ID 恢复查询，不能重复创建任务或删源。
        console.warn('HLS Downloader local file handoff awaits desktop', error)
      }
    }
  } finally {
    localDownloadsPass = false
    if (localDownloadHandoffs.size && !localDownloadsTimer) {
      localDownloadsTimer = setTimeout(() => {
        localDownloadsTimer = undefined
        void continueLocalDownloadHandoffs()
      }, 1_000)
    }
  }
}
/**
 * Chromium delivers a browser DownloadItem after the response has already
 * been accepted. Prepare request identity and policy at response time, but
 * only offer it after onCreated has paused the browser transfer. Opening the
 * desktop prompt earlier permits acceptance while the browser is still downloading.
 */
export interface EarlyBrowserTakeover {
  requestId: string
  startedAt: number
  urls: string[]
  tabId: number
  frameId: number
  promise: Promise<{ resource: MediaResource, bypass?: boolean } | null>
}
const earlyBrowserTakeovers = new Map<string, EarlyBrowserTakeover>()
const activeDownloadOffers = new Map<string, Promise<any>>()

/**
 * Locate an early takeover when the request chain is no longer available.
 * Tab navigation events routinely clear the chain between onHeadersReceived
 * and downloads.onCreated. Some browsers expose the originating tab on the
 * DownloadItem; only a unique URL-and-tab identity match is safe to reuse.
 */
function validIdentity(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) >= 0
}

export function findEarlyBrowserTakeoverByUrl(
  candidates: Array<string | undefined>,
  downloadIdentity: { tabId?: unknown, frameId?: unknown },
  entries: Iterable<EarlyBrowserTakeover> = earlyBrowserTakeovers.values(),
): EarlyBrowserTakeover | undefined {
  const wanted = candidates.filter((value): value is string => Boolean(value))
  const tabId = Number(downloadIdentity.tabId)
  if (!wanted.length || !validIdentity(tabId)) return undefined
  const frameId = Number(downloadIdentity.frameId)
  let match: EarlyBrowserTakeover | undefined
  for (const entry of entries) {
    if (entry.tabId !== tabId) continue
    if (validIdentity(frameId) && validIdentity(entry.frameId) && entry.frameId !== frameId) continue
    if (!entry.urls.some(url => wanted.includes(url))) continue
    if (match) return undefined
    match = entry
  }
  return match
}

function downloadIdentity(item: Browser.downloads.DownloadItem): { tabId?: number, frameId?: number } {
  // Chrome's DownloadItem type has no tab/frame fields, but some browsers expose
  // them at runtime. URL-only fallback is disabled when tabId is absent.
  const runtimeItem = item as Browser.downloads.DownloadItem & { tabId?: unknown, frameId?: unknown }
  const tabId = Number(runtimeItem.tabId)
  const frameId = Number(runtimeItem.frameId)
  return {
    ...(validIdentity(tabId) ? { tabId } : {}),
    ...(validIdentity(frameId) ? { frameId } : {}),
  }
}
const requestChains = new RequestChainStore()
const blobSources = new BlobSourceStore()
let nativeBridge: NativeBridge | null = null
let takeoverSettingsSync: TakeoverSettingsSync | null = null
let concealedDownloadCount = 0
let downloadUiFailsafe: ReturnType<typeof setTimeout> | null = null
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
  async get(key) {
    const local = await browser.storage.local.get(key)
    if (local[key] !== undefined) return local
    const legacy = await browser.storage.session.get(key)
    if (legacy[key] !== undefined) await browser.storage.local.set({ [key]: legacy[key] })
    return legacy
  },
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

function storageKey(tabId: number, pageUrl = '') {
  return pageResourceKey(tabId, pageUrl)
}

const responseFilename = contentDispositionFilename

async function topLevelPageUrl(tabId: number, fallback = ''): Promise<string> {
  if (tabId >= 0) {
    const tabUrl = (await browser.tabs.get(tabId).catch(() => null))?.url || ''
    if (/^https?:\/\//i.test(tabUrl)) return tabUrl
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
      tabId: validIdentity(item.tabId) ? item.tabId : undefined,
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
    if (validIdentity(tabId)) current.tabId = tabId
    scheduleHandoffPoll()
    return current
  }
  const tracked: TrackedHandoff = { id, resourceId, selectionKey, tabId, status: 'pending', checkedAt: 0, failures: 0 }
  trackedHandoffs.set(id, tracked)
  pruneTrackedHandoffs()
  await persistHandoffTracker()
  scheduleHandoffPoll()
  return tracked
}

async function pollTrackedHandoffs(): Promise<void> {
  if (handoffTrackerPolling) return
  await hydrateHandoffTracker()
  const next = [...trackedHandoffs.values()]
    .filter(item => !handoffTerminalStatus(item.status))
    .sort((left, right) => left.checkedAt - right.checkedAt)[0]
  if (!next) return
  handoffTrackerPolling = true
  try {
    const response = await native({ op: 'handoff_status', handoff_id: next.id }, 2_500)
    const handoff = response?.handoff || response
    const status = String(handoff?.status || 'pending')
    updateTrackedHandoff(next, handoff || {})
    if (handoffTerminalStatus(status)) await rememberHandoffSuppression(handoff?.suppression)
  } catch {
    next.checkedAt = Date.now()
    next.failures += 1
    if (next.failures >= 3) next.status = 'connection_lost'
  } finally {
    handoffTrackerPolling = false
    pruneTrackedHandoffs()
    await persistHandoffTracker()
    if ([...trackedHandoffs.values()].some(item => !handoffTerminalStatus(item.status))) scheduleHandoffPoll(900)
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

type BrowserFallbackDecision = DesktopTaskReadiness | 'keep-paused'

async function waitForDesktopTaskReadiness(
  handoffId: string,
  timeoutMs = 90_000,
): Promise<BrowserFallbackDecision> {
  const tracked = await trackHandoff(handoffId)
  const deadline = Date.now() + timeoutMs
  let failures = 0
  while (Date.now() < deadline) {
    try {
      const response = await native({ op: 'handoff_status', handoff_id: handoffId }, 2_500)
      const handoff = (response?.handoff || response || {}) as BrowserHandoffPayload
      updateTrackedHandoff(tracked, handoff)
      failures = 0
      const readiness = desktopTaskReadiness(handoff)
      if (readiness !== 'waiting') {
        await persistHandoffTracker()
        return readiness
      }
    } catch {
      failures += 1
      tracked.checkedAt = Date.now()
      tracked.failures = failures
    }
    await new Promise(resolve => setTimeout(resolve, failures ? 1_000 : 500))
  }
  // Status uncertainty is not permission to start a second transfer. Keep the
  // original item visibly paused so the user still owns the fallback and can
  // resume it manually; confirmed task failure is resumed automatically.
  await persistHandoffTracker()
  return 'keep-paused'
}

function schedulePausedFollowUpAccelerator(delay: number): void {
  if (pausedFollowUpAccelerator) return
  pausedFollowUpAccelerator = setTimeout(() => {
    pausedFollowUpAccelerator = null
    void continuePausedHandoffFollowups()
  }, Math.max(0, delay))
}

async function rememberPausedHandoffFollowUp(downloadId: number, handoffId: string): Promise<PausedHandoffFollowUp> {
  // Persist the deadline BEFORE the first long wait: a suspended worker drops
  // every setTimeout, so the stored record plus the worker alarm are the only
  // carriers that survive. The takeover-flow awaits are just accelerators.
  const record = await pausedFollowUpStore.remember({
    downloadId,
    handoffId,
    phase: 'resolution',
    deadline: Date.now() + PAUSED_HANDOFF_RESOLUTION_MS,
  })
  schedulePausedFollowUpAccelerator(PAUSED_HANDOFF_RESOLUTION_MS)
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
      if (pausedFollowUpsInFlight.has(record.handoffId)) continue
      pausedFollowUpsInFlight.add(record.handoffId)
      try {
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
          await pausedFollowUpStore.drop(record.handoffId)
          continue
        }
        let handoff: BrowserHandoffPayload = {}
        try {
          const response = await native({ op: 'handoff_status', handoff_id: record.handoffId }, 2_500)
          handoff = (response?.handoff || response || {}) as BrowserHandoffPayload
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
          if (resumed) await pausedFollowUpStore.drop(record.handoffId)
          else await retryPausedFollowUp(record)
          continue
        }
        concealBrowserDownload()
        try {
          const removed = await removeFollowUpDownload(record.downloadId)
          if (removed) await pausedFollowUpStore.drop(record.handoffId)
          else await retryPausedFollowUp(record)
        } finally {
          revealBrowserDownload()
        }
      } finally {
        pausedFollowUpsInFlight.delete(record.handoffId)
      }
    }
  } finally {
    pausedFollowUpPass = false
  }
  // Readiness records parked for a later re-check are pulled forward by the
  // accelerator when the worker stays alive; the alarm covers suspension.
  const parked = pausedFollowUpStore.list()
    .find(record => record.phase === 'readiness' && record.deadline > Date.now())
  if (parked) schedulePausedFollowUpAccelerator(parked.deadline - Date.now())
}

/**
 * Ownership of a paused download after the desktop accepted its handoff. The
 * persisted follow-up is the reliable carrier: only browser.alarms reliably
 * wake a suspended worker, so the shared alarm handler rehydrates and finishes
 * this cleanup. The in-process wait below only accelerates resolution while
 * this worker happens to stay alive.
 */
async function followUpPausedHandoffCleanup(item: Browser.downloads.DownloadItem, handoffId: string): Promise<void> {
  pausedFollowUpsInFlight.add(handoffId)
  try {
    const followUp = await pausedFollowUpStore.remember({
      downloadId: item.id,
      handoffId,
      phase: 'readiness',
      deadline: Date.now() + PAUSED_FOLLOW_UP_RECHECK_MS,
    })
    schedulePausedFollowUpAccelerator(PAUSED_FOLLOW_UP_RECHECK_MS)
    const later = await waitForDesktopTaskReadiness(handoffId, 180_000)
    if (later === 'safe-to-remove') {
      concealBrowserDownload()
      try {
        const removed = await removeBrowserDownload(item)
        if (removed) await pausedFollowUpStore.drop(handoffId)
        else await retryPausedFollowUp({
          ...followUp,
          phase: 'readiness',
          deadline: Date.now(),
        })
      } finally {
        revealBrowserDownload()
      }
      return
    }
    if (later === 'browser-fallback') {
      const resumed = await resumeBrowserDownload(item, true)
      if (resumed) await pausedFollowUpStore.drop(handoffId)
      else await retryPausedFollowUp({
        ...followUp,
        phase: 'readiness',
        deadline: Date.now(),
      })
    }
  } finally {
    pausedFollowUpsInFlight.delete(handoffId)
  }
  // keep-paused: the persisted follow-up keeps polling through the worker alarm.
}

async function saveResource(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1) {
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
  const key = storageKey(tabId, pageUrl)
  const merged = await resourceSessionStore.update(key, resources => mergeResources(
    resources,
    { ...resource, pageUrl, kind, id: resourceId(resource.url), seenAt: Date.now() },
    100,
    true,
  ))
  await setResourceBadge(tabId, merged)
}

function badgeText(resources: MediaResource[]): string {
  return resources.length ? String(Math.min(99, resources.length)) : ''
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
  const key = storageKey(tabId, pageUrl)
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
    'ping', 'offer', 'offer_local_file', 'handoff_status', 'media_push_status',
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

async function inspectAdaptive(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1): Promise<void> {
  if (resource.inspected) return
  const normalized = {
    ...resource,
    url: canonicalMediaUrl(resource.url, resource.kind),
  }
  const inspectionKey = `${tabId}:${normalized.pageUrl || ''}:${normalized.kind}:${normalized.url}`
  if (!['hls', 'dash'].includes(normalized.kind) || !inspectedAdaptive.claim(inspectionKey)) return
  try {
    // Manifest probes must use the same page identity as the observed request.
    // A CDN may return 403 to an extension-origin fetch even though the media
    // request in the tab succeeded. Cookies are looked up for this exact CDN
    // URL; they are never copied from the page origin to another host.
    const inspectionHeaders: Record<string, string> = {}
    const pageUrl = String(resource.pageUrl || '')
    try {
      const page = new URL(pageUrl)
      if (/^https?:$/i.test(page.protocol)) {
        inspectionHeaders.referer = page.href.split('#', 1)[0]
        inspectionHeaders.origin = page.origin
      }
    } catch {}
    const cookie = await cookiesFor(normalized.url, await topLevelPageUrl(tabId, pageUrl), tabId, resource.frameId ?? 0)
    if (cookie) inspectionHeaders.cookie = cookie
    const probeResource = { ...normalized, inspectionHeaders }
    const metadata = normalized.kind === 'hls'
      ? await inspectHlsResource(probeResource)
      : await inspectDashResource(probeResource)
    if (metadata) {
      const enriched = {
        ...normalized,
        ...metadata,
        evidence: [...new Set([...(normalized.evidence || []), 'manifest_inspection'])].slice(-16),
        owner: normalized.owner || 'manifest',
        confidence: Math.max(boundedConfidence(normalized.confidence), 0.96),
        replayContext: { ...(normalized.replayContext || {}), method: 'GET' },
      }
      await saveResource(enriched, tabId)
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
    inspectedAdaptive.defer(inspectionKey)
  }
}

async function setBrowserDownloadUi(enabled: boolean): Promise<void> {
  if (!import.meta.env.CHROME) return
  const downloads = browser.downloads as typeof browser.downloads & {
    setUiOptions?: (options: { enabled: boolean }) => Promise<void>
    setShelfEnabled?: (enabled: boolean) => Promise<void>
  }
  try {
    if (downloads.setUiOptions) await downloads.setUiOptions({ enabled })
    else if (downloads.setShelfEnabled) await downloads.setShelfEnabled(enabled)
  } catch {
    // UI suppression is best-effort; download ownership is enforced separately.
  }
}

function concealBrowserDownload(): void {
  concealedDownloadCount += 1
  void setBrowserDownloadUi(false)
  if (downloadUiFailsafe) clearTimeout(downloadUiFailsafe)
  downloadUiFailsafe = setTimeout(() => {
    concealedDownloadCount = 0
    downloadUiFailsafe = null
    void setBrowserDownloadUi(true)
  }, 130_000)
}

function revealBrowserDownload(): void {
  // 引用计数为 0 时直接返回：没有配对的 conceal 就恢复下载栏，会在 takeover
  // 删除浏览器下载项的窗口期把即将被移除的项闪回给用户。两个已知误调来源：
  // onCreated 的 `finally`（同页未被接管的下载也会走到）和 nativeBridge 的
  // `disconnected()`（单次超时重试同样会触发），见各自调用点。
  if (concealedDownloadCount <= 0) return
  concealedDownloadCount -= 1
  if (concealedDownloadCount) return
  if (downloadUiFailsafe) clearTimeout(downloadUiFailsafe)
  downloadUiFailsafe = null
  void setBrowserDownloadUi(true)
}

function successfulChainForResource(resource: MediaResource, explicitChain?: RequestChain): RequestChain | undefined {
  const isMatchingSuccess = (candidate: RequestChain | undefined): candidate is RequestChain => {
    if (!candidate || candidate.statusCode < 200 || candidate.statusCode >= 400) return false
    const finalUrl = canonicalMediaUrl(candidate.finalUrl, resource.kind)
    return resourceFingerprint({ url: finalUrl, kind: resource.kind }) === resourceFingerprint(resource)
  }
  if (isMatchingSuccess(explicitChain)) return explicitChain
  const found = resource.tabId !== undefined && resource.tabId >= 0
    ? requestChains.find({ url: resource.url, referrer: resource.pageUrl || '' }, Date.now(), resource.tabId, true)
    : requestChains.find({ url: resource.url, referrer: resource.pageUrl || '' }, Date.now(), undefined, true)
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
    chain = successfulChainForResource(resource)
  }
  throw new Error('浏览器尚未获得可用的短效签名媒体链接；请让视频继续播放或刷新页面后重试')
}

async function resourceRequest(
  resource: MediaResource,
  explicitChain?: RequestChain,
  options: { allowUnverified?: boolean } = {},
) {
  resource = { ...resource, url: canonicalMediaUrl(resource.url, resource.kind) }
  const pageUrl = await topLevelPageUrl(resource.tabId ?? -1, resource.pageUrl || '')
  const requireSuccessfulRequest = resource.kind !== 'magnet'
  let chain = usesShortLivedMediaSignature(resource)
    ? await readyResourceChain({ ...resource, pageUrl }, explicitChain)
    : explicitChain || (resource.tabId !== undefined && resource.tabId >= 0
      ? requestChains.find({ url: resource.url, referrer: pageUrl }, Date.now(), resource.tabId, requireSuccessfulRequest)
      : requestChains.find({ url: resource.url, referrer: pageUrl }, Date.now(), undefined, requireSuccessfulRequest)
    )
  if (!chain && !options.allowUnverified && resource.kind !== 'magnet') {
    // A visible URL is not by itself a browser request. Replaying a page
    // controller or stale signed gateway without a successful response chain
    // is how HTML pages became download tasks. Close the short observation
    // race, then fail closed for ordinary media/files.
    for (let attempt = 0; !chain && attempt < 8; attempt += 1) {
      await new Promise(resolve => setTimeout(resolve, 75))
      chain = resource.tabId !== undefined && resource.tabId >= 0
        ? requestChains.find({ url: resource.url, referrer: pageUrl }, Date.now(), resource.tabId, true)
        : requestChains.find({ url: resource.url, referrer: pageUrl }, Date.now(), undefined, true)
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
  options: { allowUnverified?: boolean } = {},
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
  if (resource.tabId !== undefined && resource.tabId >= 0) {
    const cookieSnapshots = new Map<string, Promise<ReplayCookie[]>>()
    const addRequestContext = async (requestUrl: string, requestHeaders: Record<string, string> | undefined) => {
      let origin = ''
      try { origin = new URL(requestUrl).origin } catch {}
      if (!origin) return
      const scopedIdentity = capturedRequestIdentity(requestHeaders, navigator.userAgent)
      let snapshot = cookieSnapshots.get(origin)
      if (!snapshot) {
        snapshot = cookieRecordsFor(requestUrl, pageUrl, resource.tabId, resource.frameId ?? 0, true)
        cookieSnapshots.set(origin, snapshot)
      }
      const cookies = await snapshot
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
      }
    }
    const contexts = requestChains.contextsForPage(resource.tabId, resource.pageUrl || pageUrl)
    // 最后应用所选媒体；同域的其他音轨/分片保留自己的目录请求身份。
    for (const context of contexts.filter(context => context.requestId !== chain?.requestId)) {
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
    resource_kind: resource.kind,
    preferred_bandwidth: resource.preferredBandwidth || 0,
    preferred_height: resource.preferredHeight || 0,
    referer: sourceIdentity.referer || identity.referer,
    origin: sourceIdentity.origin || identity.origin,
    cookie: await cookiesFor(resource.url, pageUrl, resource.tabId, resource.frameId ?? 0),
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
  options: { allowUnverified?: boolean } = {},
) {
  if (validIdentity(resource.tabId)) await browser.storage.session.remove(`download-error:${resource.tabId}`)
  await hydrateHandoffTracker()
  const selectionKey = downloadSelectionKey(resource)
  const pending = [...trackedHandoffs.values()].find(item => item.tabId === resource.tabId
    && item.resourceId === resource.id && item.status === 'pending'
    && (!item.selectionKey || item.selectionKey === selectionKey))
  if (pending) {
    const previous = await native({ op: 'handoff_status', handoff_id: pending.id })
    if (!previous?.ok) throw new Error(previous?.error || '无法读取下载确认状态')
    updateTrackedHandoff(pending, previous.handoff || {})
    await persistHandoffTracker()
    if (pending.status === 'pending') return previous
  }
  const key = `${resource.tabId ?? -1}:${resourceFingerprint(resource)}`
  const active = activeDownloadOffers.get(key)
  if (active) {
    const previous = await active
    const status = await native({ op: 'handoff_status', handoff_id: previous.handoff.id })
    if (!status?.ok) throw new Error(status?.error || '无法读取下载确认状态')
    if (status.handoff?.status === 'pending') return previous
    if (activeDownloadOffers.get(key) === active) activeDownloadOffers.delete(key)
    else return requestDownload(resource, chain, options)
  }
  const request = offer(resource, chain, { ...options, explicit: true })
    .then(response => {
      if (!response?.ok) activeDownloadOffers.delete(key)
      else void waitForHandoffResolution(String(response.handoff.id)).then(resolved => {
        if (handoffTerminalStatus(resolved.status) && activeDownloadOffers.get(key) === request) activeDownloadOffers.delete(key)
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

async function offer(resource: MediaResource, chain?: RequestChain, options: { allowUnverified?: boolean, explicit?: boolean } = {}) {
  const fingerprint = `${resource.tabId ?? -1}:${resourceFingerprint(resource)}`
  let requestId = ''
  try {
    const digest = await globalThis.crypto.subtle.digest('SHA-256', new TextEncoder().encode(fingerprint))
    requestId = `resource:${resource.tabId ?? -1}:${[...new Uint8Array(digest)].slice(0, 16).map(value => value.toString(16).padStart(2, '0')).join('')}`
  } catch {
    requestId = globalThis.crypto?.randomUUID?.()
      || `offer-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
  }
  // 只改写传输负载；原选择身份继续用于去重和弹窗恢复，避免同码率的不同线路串状态。
  const downloadResource = resource.manifestUrl && (resource.kind === 'hls' || resource.kind === 'dash')
    ? { ...resource, url: resource.manifestUrl } : resource
  const payload = {
    ...await resourcePayload(downloadResource, chain, options),
    // 自动接管按资源去重；主动点击取消/完成后允许再次打开确认窗。
    client_request_id: options.explicit ? `explicit:${crypto.randomUUID()}` : requestId,
  }
  const response = await native({ op: 'offer', resource: payload })
  const handoff = response?.handoff
  if (!response?.ok || !handoff?.id) return response
  if (handoff.presentation === 'failed' || handoff.presentation_ok === false) {
    return { ok: false, error: handoff.presentation_error || '桌面端未能打开下载确认窗口', handoff }
  }
  // Handoff creation is the acknowledgement. Presentation happens in the
  // desktop queue and must never hold the browser button at “发送中”.
  // 桌面已确认接收后，跟踪缓存写入失败不能把下载重新交还浏览器。
  await trackHandoff(String(handoff.id), resource.id, resource.tabId, downloadSelectionKey(resource))
    .catch(error => console.warn('HLS Downloader handoff tracking unavailable', error))
  return response
}

async function reportDownloadError(tabId: number | undefined, reason: unknown): Promise<void> {
  if (!validIdentity(tabId)) return
  const error = reason instanceof Error ? reason.message : String(reason || '下载请求失败，请重试')
  // 受限页面无法注入提示时，下一次打开工具栏弹窗仍能看到失败原因。
  await browser.storage.session.set({ [`download-error:${tabId}`]: error }).catch(() => undefined)
  await browser.tabs.sendMessage(tabId, { type: 'download-error', error }, { frameId: 0 }).catch(() => undefined)
}

async function refreshedDownload(downloadId: number, original: Browser.downloads.DownloadItem) {
  let determined = determinedDownloads.get(downloadId)
  if (!determined) {
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
  return { ...original, ...(current || {}), ...(determined || {}) }
}

async function removeBrowserDownload(item: Browser.downloads.DownloadItem): Promise<boolean> {
  let current: Browser.downloads.DownloadItem | undefined
  try {
    const [found] = await browser.downloads.search({ id: item.id })
    current = found
  } catch {
    return false
  }
  if (!current) return true
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
    return true
  } catch {
    // 暂停失败不能继续建立桌面任务，否则两个下载器会同时传输。
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
  if (details.statusCode < 200 || details.statusCode >= 400 || !['GET', 'POST'].includes(String(details.method || 'GET').toUpperCase())) {
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
    if (!intent && earlyTakeoverRequiresClick(resource.kind)) return null
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
    return { resource }
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
  try {
    const response = await offer(prepared.resource, chain, { explicit: true })
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

export default defineBackground(() => {
  browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
    if (changeInfo.status === 'loading' || changeInfo.url) {
      requestChains.clearTab(tabId, true)
      inspectedAdaptive.releasePrefix(`${tabId}:`)
      void browser.action.setBadgeText({ tabId, text: '' }).catch(() => undefined)
      if (changeInfo.url) {
        const keep = storageKey(tabId, changeInfo.url)
        void browser.storage.session.get(null).then(values => {
          const prefix = `resources:tab:${tabId}`
          const keys = Object.keys(values).filter(key => key.startsWith(prefix) && key !== keep)
          if (keys.length) return browser.storage.session.remove(keys)
        }).catch(() => undefined)
      }
    }
  })
  browser.tabs.onActivated.addListener(activeInfo => {
    void refreshTabBadge(activeInfo.tabId).catch(error => console.warn('HLS Downloader badge refresh failed', error))
  })
  browser.tabs.onRemoved.addListener(tabId => {
    void browser.storage.session.remove(`download-error:${tabId}`).catch(() => undefined)
    requestChains.clearTab(tabId)
    blobSources.clearTab(tabId)
    inspectedAdaptive.releasePrefix(`${tabId}:`)
    void browser.storage.session.get(null).then(values => {
      const keys = Object.keys(values).filter(key => key.startsWith(`resources:tab:${tabId}`))
      if (keys.length) return browser.storage.session.remove(keys)
    }).catch(() => undefined)
  })
  nativeBridge = new NativeBridge(
    () => browser.runtime.connectNative(HOST) as unknown as NativePortLike,
    30_000,
    () => {
      concealedDownloadCount = 0
      revealBrowserDownload()
    },
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
      const key = storageKey(tabId, pageUrl)
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
  void setBrowserDownloadUi(true)
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
    // Chromium MV3 没有异步阻塞权限，仍须在 onCreated 暂停后交接。
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
      await hydrateLocalDownloads()
      localDownloadHandoffs.set(item.id, { downloadId: item.id, requestId: `local-file:${crypto.randomUUID()}`, sourceKey, pageUrl, intent })
      await persistLocalDownloads()
      void continueLocalDownloadHandoffs()
      return
    }
    console.debug('HLS Downloader observed a browser download candidate')
    let paused = false
    let downloadUiConcealed = false
    // Once a handoff may own the transfer, finally must leave the durable
    // paused browser fallback alone. This also covers transient status loss;
    // it does not mean Core acceptance was observed.
    let browserOwnedByHandoff = false
    let followUpHandoffId = ''
    let followUpCreatedAt = 0
    try {
      const config = await settings()
      if (!config.enabled) return
      let provisionalChain = requestChains.find(originalRequest, Date.now(), blobSource?.tabId)
      // Prefer the request chain first so click matching can use tabId even when
      // Chrome leaves DownloadItem.referrer empty. After a click is known, re-bind
      // the chain to that tab so we never replay another page's auth headers.
      const earlyTakeover = (provisionalChain
        ? earlyBrowserTakeovers.get(provisionalChain.requestId)
        : undefined)
        ?? findEarlyBrowserTakeoverByUrl(
          [(item as any).finalUrl, item.url, originalRequest.url],
          downloadIdentity(item),
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
          const pauseResult = await pauseBrowserDownload(item)
          if (pauseResult === null) return
          paused = pauseResult
          concealBrowserDownload()
          downloadUiConcealed = true
          const response = earlyResult
            ? await offer(earlyResult.resource, provisionalChain) : undefined
          if (!restored && !desktopAcceptedHandoff(response)) return
          const handoffId = String(response?.handoff?.id || restored?.id || '')
          if (earlyTakeover && earlyResult && response?.handoff?.id) {
            const tracked = await trackHandoff(handoffId, earlyResult.resource.id, earlyResult.resource.tabId)
            tracked.browserRequestId = earlyTakeover.requestId
            await persistHandoffTracker()
          }
          followUpHandoffId = handoffId
          followUpCreatedAt = (await rememberPausedHandoffFollowUp(item.id, handoffId)).createdAt
          const handoff = await waitForHandoffResolution(handoffId)
          if (handoff?.status !== 'accepted') {
            if (!handoffTerminalStatus(handoff?.status)) browserOwnedByHandoff = true
            return
          }
          const readiness = await waitForDesktopTaskReadiness(handoffId)
          if (readiness === 'browser-fallback') return
          if (readiness === 'keep-paused') {
            browserOwnedByHandoff = true
            void followUpPausedHandoffCleanup(item, handoffId)
            return
          }
          const removed = await removeBrowserDownload(item)
          browserOwnedByHandoff = true
          if (removed) await pausedFollowUpStore.drop(handoffId)
          return
        }
      }
      // Filename determination is useful for generated downloads but can take
      // hundreds of milliseconds. A zip/exe/pdf URL is already a file: skip
      // that wait so the confirm window can appear as soon as Chrome pauses.
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
      if (!item.filename) {
        actualBrowser = await refreshedDownload(item.id, item)
        if (!canContinueTakeover(actualBrowser.state, paused)) return
        provisionalChain = requestChains.find(downloadRequestItem(actualBrowser, blobSource), Date.now(), blobSource?.tabId) || provisionalChain
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
      let chain = intent?.tabId === undefined
        ? provisionalChain
        : requestChains.find(classified.actual, Date.now(), intent.tabId) || provisionalChain
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
      // DownloadItem 可能先于请求链到达。使用与 offer 相同的解析流程确认方法后
      // 才暂停，且把同一条链传入 offer，避免晚到的 POST 被当成 GET 路径重放。
      chain = (await resourceRequest(resource, chain)).chain
      if (!chain) return
      if (chain.method.toUpperCase() === 'POST') {
        // 复用本地导入，复制成功前保留浏览器源文件。
        await hydrateLocalDownloads()
        localDownloadHandoffs.set(item.id, {
          downloadId: item.id, requestId: `local-file:${crypto.randomUUID()}`,
          sourceKey: await localDownloadIntentUrl(item.url), pageUrl: resource.pageUrl || '', intent,
          importUrl: 'data:application/octet-stream,',
        })
        await persistLocalDownloads()
        void continueLocalDownloadHandoffs()
        return
      }
      const pauseResult = await pauseBrowserDownload(actualBrowser)
      if (pauseResult === null) return
      paused = pauseResult
      concealBrowserDownload()
      downloadUiConcealed = true
      console.debug('HLS Downloader offering a verified browser download')
      const response = await offer(resource, chain)
      if (!desktopAcceptedHandoff(response)) throw new Error(response?.error || 'desktop rejected')
      // Do not discard the browser download just because the confirmation
      // window opened. The user owns the final decision; cancel/reject keeps
      // this original download in the browser. The follow-up deadline is
      // persisted before waiting so a suspended worker continues this flow
      // from the shared alarm.
      const handoffId = String(response.handoff.id)
      followUpHandoffId = handoffId
      followUpCreatedAt = (await rememberPausedHandoffFollowUp(item.id, handoffId)).createdAt
      const handoff = await waitForHandoffResolution(handoffId)
      if (handoff?.status !== 'accepted') {
        if (!handoffTerminalStatus(handoff?.status)) browserOwnedByHandoff = true
        return
      }
      const readiness = await waitForDesktopTaskReadiness(handoffId)
      if (readiness === 'browser-fallback') return
      if (readiness === 'keep-paused') {
        browserOwnedByHandoff = true
        void followUpPausedHandoffCleanup(actualBrowser, handoffId)
        return
      }
      // Core 已有实际传输后，取消并移除原下载项；确认框关闭或接管失败则恢复原下载。
      const removed = await removeBrowserDownload(actualBrowser)
      browserOwnedByHandoff = true
      if (removed) await pausedFollowUpStore.drop(handoffId)
    } catch (error) {
      console.warn('HLS Downloader takeover failed; browser download remains untouched', error)
    } finally {
      determinedDownloads.delete(item.id)
      determinationWaiters.delete(item.id)
      // A failed browser recovery keeps its record so the alarm can retry it.
      if (followUpHandoffId && !browserOwnedByHandoff) {
        const resumed = await resumeBrowserDownload(item, paused)
        if (resumed) await pausedFollowUpStore.drop(followUpHandoffId)
        else await retryPausedFollowUp({
          downloadId: item.id,
          handoffId: followUpHandoffId,
          phase: 'resolution',
          deadline: Date.now(),
          createdAt: followUpCreatedAt,
        })
      } else {
        await resumeBrowserDownload(item, paused && !browserOwnedByHandoff)
      }
      if (downloadUiConcealed) revealBrowserDownload()
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
      const resource = {
        ...message.resource,
        pageUrl: message.resource.pageUrl || sender.url || sender.tab?.url,
        frameId: message.resource.frameId ?? sender.frameId,
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
        .then(pageUrl => storageKey(tabId, pageUrl))
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
