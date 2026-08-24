import { browser } from 'wxt/browser'
import { mediaPushRequestId } from '../lib/mediaPush'
import { isSniffCurrentPageCommand, openMediaPanelMessage } from '../lib/sniffCommand'
import { NativeBridge, type NativePortLike } from '../lib/nativeBridge'
import { boundedConfidence, canonicalMediaUrl, capturedRequestIdentity, classifyDownload, classifyPlaybackSource, classifyResource, compactResources, isConcreteDownloadMime, isShortLivedMediaSignatureUsable, mergeResources, normalizeHost, pageResourceKey, pruneExpiredResources, replayableRequestHeaders, resourceBelongsToFrame, resourceFingerprint, resourceId, resourceRequestIdentity, shouldTakeover, suggestedResourceFilename, usesShortLivedMediaSignature, type DownloadClickIntent, type MediaResource } from '../lib/resources'
import { RequestChainStore, replayablePostRequest, requestHeader, responseHeader, type RequestChain } from '../lib/requestChain'
import { browserCleanupAction, canContinueTakeover, canResumeBrowserDownload, desktopAcceptedHandoff, desktopTaskReadiness, handoffStatusLabel, handoffTerminalStatus, type BrowserHandoffPayload, type DesktopTaskReadiness } from '../lib/takeover'
import { HANDOFF_SUPPRESSION_STORAGE_KEY, isHandoffSuppressed, normalizeHandoffSuppressions } from '../lib/handoffSuppression'
import { filenameDeterminationEvent, requestHeaderExtraInfo } from '../lib/browserCapabilities'
import { inspectHlsResource } from '../lib/hlsInspection'
import { inspectDashResource } from '../lib/dashInspection'
import { contentDispositionFilename } from '../lib/contentDisposition'
import { InspectionCache } from '../lib/inspectionCache'
import { cookieLookupUrl, cookiePermissionAllows, normalizeCookiePermissionHosts } from '../lib/browserCookies'
import { detectBrowserFamily, stableBrowserClientId } from '../lib/browserClient'
import { BrowserDirectBackend, shouldAttachLoopbackBridge, shouldClearLoopbackBridge, shouldRouteThroughLoopbackBridge } from '../lib/directBackend'
import { isEarlyDirectDownloadResponse, type ObservedDownloadResource } from '../lib/directResponse'
import { clickIntentPollsForKind, earlyTakeoverRequiresClick } from '../lib/fileTakeover'
import { ClickIntentStore } from '../lib/clickIntentStore'
import { TakeoverSettingsSync } from '../lib/takeoverSettingsSync'
import { SessionListStore } from '../lib/sessionListStore'
import { BlobSourceStore, type BlobSourceRecord } from '../lib/blobSourceStore'
import { contextMenuCapabilities } from '../lib/contextMenuActions'

const HOST = 'com.ciaooo55.hls_downloader'
const dynamicContextMenus = browser.contextMenus as typeof browser.contextMenus & {
  onShown?: { addListener: (listener: (info: { linkUrl?: string, srcUrl?: string, mediaType?: string }) => void) => void }
  refresh?: () => Promise<void> | void
}
const CLICK_INTENT_STORAGE_KEY = 'click-intents'
const clickIntentStore = new ClickIntentStore(browser.storage.session, CLICK_INTENT_STORAGE_KEY)
let browserFallbacks: Array<{ url: string, at: number }> = []
const MAX_BROWSER_FALLBACKS = 128
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
/**
 * Chrome MV3 delivers a browser DownloadItem after the response has already
 * been accepted.  Keep a short-lived promise for the direct responses that
 * were offered from onHeadersReceived so onCreated can reuse that handoff
 * instead of waiting for a second offer round.  Firefox does not use this
 * path because its blocking listener can cancel the response directly.
 */
interface EarlyBrowserTakeover {
  requestId: string
  startedAt: number
  urls: string[]
  promise: Promise<{ resource: MediaResource, response: any } | null>
}
const earlyBrowserTakeovers = new Map<string, EarlyBrowserTakeover>()

/**
 * Locate an early takeover when the request chain is no longer available.
 * Tab navigation events routinely clear the chain between onHeadersReceived
 * and downloads.onCreated; matching by URL prevents offering the same
 * download to the desktop twice (duplicate confirmation/task).
 */
function findEarlyBrowserTakeoverByUrl(candidates: Array<string | undefined>): EarlyBrowserTakeover | undefined {
  const wanted = candidates.filter((value): value is string => Boolean(value))
  if (!wanted.length) return undefined
  for (const entry of earlyBrowserTakeovers.values()) {
    if (entry.urls.some(url => wanted.includes(url))) return entry
  }
  return undefined
}
const requestChains = new RequestChainStore()
const blobSources = new BlobSourceStore()
let nativeBridge: NativeBridge | null = null
let directBackend: BrowserDirectBackend | null = null
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

async function trackHandoff(handoffId: string, resourceId = ''): Promise<TrackedHandoff> {
  await hydrateHandoffTracker()
  const id = String(handoffId || '')
  const current = trackedHandoffs.get(id)
  if (current) {
    if (resourceId && !current.resourceId) current.resourceId = resourceId
    scheduleHandoffPoll()
    return current
  }
  const tracked: TrackedHandoff = { id, resourceId, status: 'pending', checkedAt: 0, failures: 0 }
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
  return { id: handoffId, resourceId: '', status: 'expired', checkedAt: Date.now(), failures: 0 }
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

function followUpPausedHandoffCleanup(item: Browser.downloads.DownloadItem, handoffId: string): void {
  void waitForDesktopTaskReadiness(handoffId, 180_000).then(async later => {
    if (later === 'safe-to-remove') {
      concealBrowserDownload()
      try {
        await removeBrowserDownload(item)
      } finally {
        revealBrowserDownload()
      }
      return
    }
    if (later === 'browser-fallback') {
      await resumeBrowserDownload(item, true)
      revealBrowserDownload()
      return
    }
    followUpPausedHandoffCleanup(item, handoffId)
  }).catch(() => undefined)
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

async function cookiesFor(url: string, pageUrl = ''): Promise<string> {
  const cookieUrl = cookieLookupUrl(url)
  if (!cookieUrl) return ''
  const config = await settings()
  // Authorizing a page means its detected resources may reuse only cookies
  // that the browser would send to the resource URL itself. Page cookies are
  // never copied across origins.
  if (!cookiePermissionAllows(cookieUrl, pageUrl, config.authorizedCookieHosts)) return ''
  const values = await browser.cookies.getAll({ url: cookieUrl }).catch(() => [])
  return values.map(cookie => `${cookie.name}=${cookie.value}`).join('; ')
}

async function native(message: Record<string, unknown>, timeoutMs?: number): Promise<any> {
  if (!nativeBridge) return Promise.reject(new Error('插件连接尚未初始化'))
  const identity = extensionIdentity()
  const operation = String(message.op || '')
  const retryCount = new Set([
    'ping', 'offer', 'handoff_status', 'media_push_status',
  ]).has(operation) ? 1 : 0
  const payload = {
    ...message,
    version: identity.version,
    client_id: await browserClientId(),
    browser: identity.browser,
  }
  if (directBackend && shouldRouteThroughLoopbackBridge(operation, true)) {
    try {
      return await directBackend.request(payload, {
        version: identity.version,
        client_id: String(payload.client_id),
        browser: identity.browser,
      }, timeoutMs)
    } catch {
      // Core restart, token rotation or an older backend: renew pairing over
      // Native Messaging without surfacing a false "未连接" state.
      directBackend = null
    }
  }
  const response = await nativeBridge.request(payload, timeoutMs, retryCount)
  if (shouldClearLoopbackBridge(response)) {
    directBackend = null
  } else if (shouldAttachLoopbackBridge(response)) {
    directBackend = new BrowserDirectBackend(String(response.bridge_base), String(response.bridge_token))
  }
  return response
}

async function pingDesktop(): Promise<any> {
  const response = takeoverSettingsSync
    ? await takeoverSettingsSync.applyPing(await native({ op: 'ping' }))
    : await native({ op: 'ping' })
  if (response?.ok) lastDesktopPingAt = Date.now()
  return response
}

async function inspectAdaptive(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1): Promise<void> {
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
    const cookie = await cookiesFor(normalized.url, pageUrl)
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
  concealedDownloadCount = Math.max(0, concealedDownloadCount - 1)
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

async function resourcePayload(
  resource: MediaResource,
  explicitChain?: RequestChain,
  options: { allowUnverified?: boolean } = {},
) {
  resource = { ...resource, url: canonicalMediaUrl(resource.url, resource.kind) }
  const pageUrl = await topLevelPageUrl(resource.tabId ?? -1, resource.pageUrl || '')
  const pageChain = resource.tabId !== undefined && resource.tabId >= 0
    ? requestChains.pageContext(resource.tabId, pageUrl)
    : undefined
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
    const addRequestContext = async (requestUrl: string, requestHeaders: Record<string, string> | undefined) => {
      let origin = ''
      try { origin = new URL(requestUrl).origin } catch {}
      if (!origin) return
      const scopedIdentity = capturedRequestIdentity(requestHeaders, navigator.userAgent)
      requestContexts[origin] = {
        request_headers: replayableRequestHeaders(requestHeaders),
        referer: scopedIdentity.referer,
        origin: scopedIdentity.origin,
        user_agent: scopedIdentity.userAgent,
        cookie: await cookiesFor(requestUrl, pageUrl),
      }
    }
    const contexts = requestChains.contextsForPage(resource.tabId, pageUrl)
    // An iframe resource has its own document URL and is deliberately absent
    // from contextsForPage(topPage). The selected resource's exact chain is
    // still authoritative for its CDN credentials, so append it explicitly.
    if (chain && !contexts.some(context => context.requestId === chain.requestId)) contexts.push(chain)
    for (const context of contexts) {
      await addRequestContext(context.finalUrl, context.requestHeaders)
    }
    if (!chain && resource.requestHeaders && Object.keys(resource.requestHeaders).length) {
      // Variant URLs parsed from a master HLS playlist are often chosen before
      // the browser fetches them. Reuse that master request's captured headers
      // only for the selected variant's origin, and fetch cookies for that URL.
      await addRequestContext(resource.url, resource.requestHeaders)
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
    referer: sourceIdentity.referer || identity.referer,
    origin: sourceIdentity.origin || identity.origin,
    // This top-level context belongs to the browser URL, not the media URL.
    // Exact resource/CDN cookies are still supplied through request_contexts.
    cookie: await cookiesFor(pageUrl || resource.url, pageUrl),
    user_agent: sourceIdentity.userAgent || identity.userAgent,
    request_headers: replayableRequestHeaders(pageChain?.requestHeaders || resource.requestHeaders),
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

async function downloadNow(
  resource: MediaResource,
  chain?: RequestChain,
  options: { allowUnverified?: boolean } = {},
) {
  const payload = await resourcePayload(resource, chain, options)
  return native({ op: 'download', resource: payload })
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

async function offer(resource: MediaResource, chain?: RequestChain, options: { allowUnverified?: boolean } = {}) {
  const fingerprint = `${resource.tabId ?? -1}:${resourceFingerprint(resource)}`
  let requestId = ''
  try {
    const digest = await globalThis.crypto.subtle.digest('SHA-256', new TextEncoder().encode(fingerprint))
    requestId = `resource:${resource.tabId ?? -1}:${[...new Uint8Array(digest)].slice(0, 16).map(value => value.toString(16).padStart(2, '0')).join('')}`
  } catch {
    requestId = globalThis.crypto?.randomUUID?.()
      || `offer-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
  }
  const payload = {
    ...await resourcePayload(resource, chain, options),
    client_request_id: requestId,
  }
  const response = await native({ op: 'offer', resource: payload })
  const handoff = response?.handoff
  if (!response?.ok || !handoff?.id) return response
  if (handoff.presentation === 'failed' || handoff.presentation_ok === false) {
    return { ok: false, error: handoff.presentation_error || '桌面端未能打开下载确认窗口', handoff }
  }
  // Handoff creation is the acknowledgement. Presentation happens in the
  // desktop queue and must never hold the browser button at “发送中”.
  void trackHandoff(String(handoff.id), resource.id)
  return response
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

async function removeBrowserDownload(item: Browser.downloads.DownloadItem): Promise<void> {
  const [current] = await browser.downloads.search({ id: item.id })
  const state = current?.state || item.state
  if (browserCleanupAction(state) === 'remove-file') {
    await browser.downloads.removeFile(item.id).catch(() => undefined)
  } else {
    await browser.downloads.cancel(item.id).catch(() => undefined)
  }
  await browser.downloads.erase({ id: item.id }).catch(() => undefined)
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

async function pauseBrowserDownload(item: Browser.downloads.DownloadItem): Promise<boolean> {
  if (item.state !== 'in_progress') return false
  try {
    await browser.downloads.pause(item.id)
    return true
  } catch {
    return false
  }
}

async function resumeBrowserDownload(item: Browser.downloads.DownloadItem, paused: boolean): Promise<void> {
  if (!paused) return
  const [current] = await browser.downloads.search({ id: item.id }).catch(() => [])
  // Edge/Chrome may report a just-paused response as `interrupted` while it is
  // still resumable. Refusing that state left excluded downloads and failed or
  // rejected handoffs stuck forever at 0 B.
  if (!current || !canResumeBrowserDownload(current.state)) return
  try { await browser.downloads.resume(item.id) } catch {}
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
    })
    if (intent) return intent
    if (attempt + 1 < polls) await new Promise(resolve => setTimeout(resolve, 25))
  }
  return undefined
}

function consumeBrowserFallback(url: string): boolean {
  const now = Date.now()
  browserFallbacks = browserFallbacks
    .filter(item => now - item.at <= 7000)
    .slice(0, MAX_BROWSER_FALLBACKS)
  const index = browserFallbacks.findIndex(item => item.url === url)
  if (index < 0) return false
  browserFallbacks.splice(index, 1)
  return true
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

async function startBrowserFallback(url: string, filename = ''): Promise<number> {
  revealBrowserDownload()
  const now = Date.now()
  browserFallbacks = browserFallbacks
    .filter(item => now - item.at <= 7000)
    .slice(0, MAX_BROWSER_FALLBACKS - 1)
  browserFallbacks.unshift({ url, at: now })
  try {
    return await browser.downloads.download({ url, ...(filename ? { filename } : {}) })
  } catch (error) {
    consumeBrowserFallback(url)
    throw error
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
  const kind = disposition
    || mimeType.toLowerCase().includes('octet-stream')
    || isConcreteDownloadMime(mimeType)
    ? classifyDownload(details.url, mimeType, filename, disposition)
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
  void saveResource(resource, details.tabId)
  void inspectAdaptive(resource, details.tabId)
  void sendCapturedResource(details.tabId, resource)
  return { disposition, resource }
}

function trackedSize(chain: RequestChain | undefined): number {
  const contentRange = responseHeader(chain, 'content-range')
  const rangeTotal = Number(contentRange.match(/\/(\d+)$/)?.[1] || 0)
  return rangeTotal || Number(responseHeader(chain, 'content-length') || 0)
}

function rememberEarlyBrowserTakeover(details: any, chain: RequestChain | undefined, observed: { disposition: string, resource: ObservedDownloadResource | null }): void {
  if (!chain || !observed.resource || !isEarlyDirectDownloadResponse(details, observed)) return
  const observedResource = observed.resource
  const requestId = String(chain.requestId || details.requestId || '')
  if (!requestId || earlyBrowserTakeovers.has(requestId)) return
  const promise = (async () => {
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
      )
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
      const response = await offer(resource, chain)
      // Keep a response with a handoff id even when its presentation was
      // rejected; onCreated must not issue a duplicate desktop task in that
      // case.  A missing id means the early attempt never reached the app and
      // the normal DownloadItem path may retry safely.
      return response?.handoff?.id ? { resource, response } : null
    } catch (error) {
      console.debug('HLS Downloader early browser takeover unavailable', error)
      return null
    }
  })()
  const urls = [...new Set([observedResource.url, String(details.url || '')].filter(Boolean))]
  earlyBrowserTakeovers.set(requestId, { requestId, startedAt: Date.now(), urls, promise })
  void promise.finally(() => {
    setTimeout(() => {
      const current = earlyBrowserTakeovers.get(requestId)
      if (current?.promise === promise) earlyBrowserTakeovers.delete(requestId)
    }, 30_000)
  })
  // Keep the map bounded on pages that repeatedly navigate through downloads.
  if (earlyBrowserTakeovers.size > 128) {
    const oldest = [...earlyBrowserTakeovers.values()]
      .sort((left, right) => left.startedAt - right.startedAt)[0]
    if (oldest) earlyBrowserTakeovers.delete(oldest.requestId)
  }
}

export default defineBackground(() => {
  browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
    if (changeInfo.status === 'loading' || changeInfo.url) {
      requestChains.clearTab(tabId)
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
    void refreshTabBadge(activeInfo.tabId)
  })
  browser.tabs.onRemoved.addListener(tabId => {
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
  void hydrateHandoffTracker().then(() => pollTrackedHandoffs()).catch(() => undefined)
  void setBrowserDownloadUi(true)
  void pingDesktop().catch(() => undefined)
  browser.alarms.create('desktop-heartbeat', { periodInMinutes: 0.5 })
  browser.alarms.create('handoff-tracker', { periodInMinutes: 0.5 })
  // Reconcile inactive tabs without waking the worker on every heartbeat.
  // Activation remains immediate; this only expires a badge on a tab that
  // has not been selected for a long time.
  browser.alarms.create('resource-badge-refresh', { periodInMinutes: 5 })
  browser.alarms.onAlarm.addListener(alarm => {
    if (alarm.name === 'desktop-heartbeat') {
      requestChains.cleanup()
      blobSources.cleanup()
      void pingDesktop().catch(() => undefined)
    }
    if (alarm.name === 'handoff-tracker') void pollTrackedHandoffs()
    if (alarm.name === 'resource-badge-refresh') void refreshOpenTabBadges()
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
      void browser.tabs.sendMessage(tab.id, openMediaPanelMessage()).catch(() => undefined)
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

  browser.webRequest.onHeadersReceived.addListener(details => {
    const chain = requestChains.observeResponse(details as any)
    const observed = observedResponse(details, chain)
    // Offer as soon as response headers prove a download, but never cancel
    // the navigation here. Firefox used to {cancel:true} once the desktop
    // window opened; rejecting that window then left no DownloadItem to
    // resume. onCreated pauses the browser item and either removes it after
    // a successful desktop transfer or resumes it on reject/expire.
    rememberEarlyBrowserTakeover(details, chain, observed)
    return undefined
  }, TRACKED_REQUEST_FILTER, ['responseHeaders'])
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
    // A client-generated Blob has no replayable HTTP origin. Leave it entirely
    // browser-owned. Fetched blobs proceed only when the page hook correlated
    // this exact object URL to its successful response.
    if (item.url.startsWith('blob:') && !blobSource) return
    const originalRequest = downloadRequestItem(item, blobSource)
    const creatingExtension = String((item as any).byExtensionId || '')
    if (creatingExtension && creatingExtension !== browser.runtime.id) return
    if (consumeBrowserFallback(originalRequest.url)) {
      revealBrowserDownload()
      return
    }
    console.debug('HLS Downloader observed a browser download candidate')
    let paused = false
    let accepted = false
    try {
      // IDM pauses the browser item immediately in onCreated and resolves
      // ownership afterwards. Do the same before any storage/native await so a
      // fast local/CDN file cannot visibly advance behind the desktop prompt.
      paused = await pauseBrowserDownload(item)
      const config = await settings()
      if (!config.enabled) return
      // Prefer the request chain first so click matching can use tabId even when
      // Chrome leaves DownloadItem.referrer empty. After a click is known, re-bind
      // the chain to that tab so we never replay another page's auth headers.
      let provisionalChain = requestChains.find(originalRequest, Date.now(), blobSource?.tabId)
      const earlyTakeover = (provisionalChain
        ? earlyBrowserTakeovers.get(provisionalChain.requestId)
        : undefined)
        ?? findEarlyBrowserTakeoverByUrl([(item as any).finalUrl, item.url, originalRequest.url])
      if (earlyTakeover) {
        const earlyResult = await earlyTakeover.promise
        earlyBrowserTakeovers.delete(earlyTakeover.requestId)
        if (earlyResult?.response?.handoff?.id) {
          // The early response already created the desktop handoff.  If the
          // desktop rejected presentation, leave the original browser item
          // untouched; otherwise wait for the user's final decision and clean
          // up the browser item only after acceptance.
          if (!desktopAcceptedHandoff(earlyResult.response)) return
          const handoff = await waitForHandoffResolution(String(earlyResult.response.handoff.id))
          if (handoff?.status !== 'accepted') return
          const readiness = await waitForDesktopTaskReadiness(String(earlyResult.response.handoff.id))
          if (readiness === 'browser-fallback') return
          if (readiness === 'keep-paused') {
            accepted = true
            followUpPausedHandoffCleanup(item, String(earlyResult.response.handoff.id))
            return
          }
          concealBrowserDownload()
          await removeBrowserDownload(item)
          accepted = true
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
          kind: classifyDownload(url, mimeType, filename, contentDisposition),
        }
      }
      let classified = classifyBrowserItem(item, provisionalChain)
      let actualBrowser = item
      if (!classified.kind) {
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
      if (String(chain?.method || '').toUpperCase() === 'POST' && !replayablePostRequest(chain).request_body) {
        return
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
      console.debug('HLS Downloader offering a verified browser download')
      const response = await offer(resource, chain)
      if (!desktopAcceptedHandoff(response)) throw new Error(response?.error || 'desktop rejected')
      // Do not discard the browser download just because the confirmation
      // window opened. The user owns the final decision; cancel/reject keeps
      // this original download in the browser.
      const handoff = await waitForHandoffResolution(String(response.handoff.id))
      if (handoff?.status !== 'accepted') return
      const readiness = await waitForDesktopTaskReadiness(String(response.handoff.id))
      if (readiness === 'browser-fallback') return
      if (readiness === 'keep-paused') {
        accepted = true
        followUpPausedHandoffCleanup(actualBrowser, String(response.handoff.id))
        return
      }
      // Do not hide Chrome's downloads UI merely because a browser download was
      // observed. Suppress it only after the desktop accepted the handoff.
      concealBrowserDownload()
      await removeBrowserDownload(actualBrowser)
      accepted = true
    } catch (error) {
      console.warn('HLS Downloader takeover failed; browser download remains untouched', error)
    } finally {
      determinedDownloads.delete(item.id)
      determinationWaiters.delete(item.id)
      await resumeBrowserDownload(item, paused && !accepted)
      revealBrowserDownload()
    }
  })

  browser.contextMenus.onClicked.addListener((info, tab) => {
    if (info.menuItemId === 'hls-download-selection') {
      if (tab?.id !== undefined) void browser.tabs.sendMessage(tab.id, { type: 'collect-selection' }).catch(() => undefined)
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
        const frameId = Number((info as any).frameId)
        void browser.tabs.sendMessage(
          tab.id,
          { type: 'open-media-panel' },
          Number.isInteger(frameId) && frameId >= 0 ? { frameId } : undefined,
        ).catch(() => undefined)
      }
      return
    }
    const resource = {
      id: resourceId(url), url, kind, pageUrl: tab?.url, title: tab?.title, tabId: tab?.id,
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
    void downloadNow(resource, undefined, { allowUnverified: true })
      .catch(error => console.warn('HLS Downloader context download failed', error))
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
      void inspectAdaptive(resource, sender.tab?.id ?? -1)
      return
    }
    if (message?.type === 'download-now') {
      const resource = {
        ...message.resource,
        pageUrl: message.resource.pageUrl || sender.url || sender.tab?.url || '',
        tabId: message.resource.tabId ?? sender.tab?.id,
        frameId: message.resource.frameId ?? sender.frameId,
      }
      // A click on our popup/hover action is already an explicit confirmation.
      // Create the task directly; automatic browser takeover continues to use
      // the separate desktop confirmation window.
      const explicitSelection = resource.owner === 'selection'
        && Array.isArray(resource.evidence)
        && resource.evidence.includes('text_selection')
      void downloadNow(resource, undefined, { allowUnverified: explicitSelection })
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'download' || message?.type === 'offer') {
      const resource = {
        ...message.resource,
        pageUrl: message.resource.pageUrl || sender.url || sender.tab?.url || '',
        tabId: message.resource.tabId ?? sender.tab?.id,
        frameId: message.resource.frameId ?? sender.frameId,
      }
      const fromPage = /^https?:\/\//i.test(String(sender.url || ''))
      const request = fromPage || message.type === 'offer' ? offer(resource) : downloadNow(resource)
      void request
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'handoff-status') {
      void handoffStatus(String(message.handoffId || message.handoff_id || ''))
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'browser-download') {
      const url = String(message.url || '')
      void startBrowserFallback(url, String(message.filename || ''))
        .then(downloadId => sendResponse({ ok: true, downloadId }))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'list') {
      const tabId = Number(message.tabId ?? sender.tab?.id ?? -1)
      // Content scripts run in every frame, while the session cache is kept
      // page-scoped so the popup can aggregate all players.  A frame overlay
      // must receive only its own observations; otherwise two same-page MSE
      // players (especially in cross-origin iframes) can exchange manifests
      // and show a download button for the wrong video.  Extension pages such
      // as the popup do not have an http(s) sender URL, so they intentionally
      // keep the aggregate view.
      const pageSender = /^https?:\/\//i.test(String(sender.url || ''))
      const senderFrameId = pageSender && Number.isInteger(Number(sender.frameId))
        ? Number(sender.frameId)
        : -1
      void topLevelPageUrl(tabId, String(message.pageUrl || ''))
        .then(pageUrl => storageKey(tabId, pageUrl))
        .then(key => resourceSessionStore.update(key, raw => compactResources(pruneExpiredResources(raw), 40, true)).then(async cleaned => {
          // Never write the frame-filtered view back to the page cache: doing
          // so would erase sibling iframe resources just because one frame
          // opened its panel first.
          const scoped = senderFrameId >= 0
            ? cleaned.filter(resource => resourceBelongsToFrame(resource, senderFrameId))
            : cleaned
          const visible = senderFrameId >= 0 ? compactResources(scoped, 40, true) : compactResources(cleaned, 40)
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
