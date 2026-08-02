import { browser } from 'wxt/browser'
import { NativeBridge, type NativePortLike } from '../lib/nativeBridge'
import { canonicalMediaUrl, capturedRequestIdentity, classifyDownload, classifyResource, compactResources, matchesDownloadClick, mergeResources, pageResourceKey, replayableRequestHeaders, resourceFingerprint, resourceId, resourceRequestIdentity, shouldTakeover, suggestedResourceFilename, type DownloadClickIntent, type MediaResource } from '../lib/resources'
import { RequestChainStore, replayablePostRequest, requestHeader, responseHeader, type RequestChain } from '../lib/requestChain'
import { browserCleanupAction, canContinueTakeover, desktopAcceptedHandoff, handoffStatusLabel, handoffTerminalStatus } from '../lib/takeover'
import { HANDOFF_SUPPRESSION_STORAGE_KEY, isHandoffSuppressed, normalizeHandoffSuppressions } from '../lib/handoffSuppression'
import { filenameDeterminationEvent, requestHeaderExtraInfo, resolveFirefoxClickIntent } from '../lib/browserCapabilities'
import { inspectHlsResource } from '../lib/hlsInspection'
import { contentDispositionFilename } from '../lib/contentDisposition'
import { InspectionCache } from '../lib/inspectionCache'
import { cookieLookupUrl } from '../lib/browserCookies'
import { detectBrowserFamily, stableBrowserClientId } from '../lib/browserClient'
import { BrowserDirectBackend } from '../lib/directBackend'

const HOST = 'com.ciaooo55.hls_downloader'
const CLICK_INTENT_STORAGE_KEY = 'click-intents'
let clickIntents: DownloadClickIntent[] = []
let clickIntentsHydrated = false
let browserFallbacks: Array<{ url: string, at: number }> = []
const determinedDownloads = new Map<number, Browser.downloads.DownloadItem>()
const determinationWaiters = new Map<number, (item: Browser.downloads.DownloadItem) => void>()
const requestChains = new RequestChainStore()
let nativeBridge: NativeBridge | null = null
let directBackend: BrowserDirectBackend | null = null
let concealedDownloadCount = 0
let downloadUiFailsafe: ReturnType<typeof setTimeout> | null = null
const inspectedHls = new InspectionCache()
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
}

const trackedHandoffs = new Map<string, TrackedHandoff>()
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

async function settings() {
  const data = await browser.storage.local.get([
    'enabled', 'minimumBytes', 'excludedHosts', 'authorizedCookieHosts',
    'useBrowserCookies', HANDOFF_SUPPRESSION_STORAGE_KEY,
  ])
  return {
    enabled: data.enabled !== false,
    minimumBytes: Number(data.minimumBytes ?? 0),
    excludedHosts: Array.isArray(data.excludedHosts) ? data.excludedHosts : [],
    authorizedCookieHosts: Array.isArray(data.authorizedCookieHosts) ? data.authorizedCookieHosts : [],
    suppressions: normalizeHandoffSuppressions(data[HANDOFF_SUPPRESSION_STORAGE_KEY]),
    // Media URLs commonly require the logged-in browser session. This is on by
    // default; cookies are read only for the exact resource when the user
    // explicitly sends it to the desktop app, never while merely sniffing.
    useBrowserCookies: data.useBrowserCookies !== false,
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
    })
  }
}

async function persistHandoffTracker(): Promise<void> {
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
    next.status = status
    next.checkedAt = Date.now()
    next.failures = 0
    next.presentation = typeof handoff?.presentation === 'string' ? handoff.presentation : next.presentation
    next.suppression = handoff?.suppression
    if (handoffTerminalStatus(status)) await rememberHandoffSuppression(handoff?.suppression)
  } catch {
    next.checkedAt = Date.now()
    next.failures += 1
    if (next.failures >= 3) next.status = 'connection_lost'
  } finally {
    handoffTrackerPolling = false
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
  const stored = await browser.storage.session.get(key)
  const resources = Array.isArray(stored[key]) ? stored[key] : []
  const merged = mergeResources(resources, { ...resource, pageUrl, kind, id: resourceId(resource.url), seenAt: Date.now() })
  await browser.storage.session.set({ [key]: merged })
  await browser.action.setBadgeText({ text: String(Math.min(99, merged.length)), ...(tabId >= 0 ? { tabId } : {}) })
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
  const host = new URL(cookieUrl).host
  let pageHost = ''
  try {
    const lookup = cookieLookupUrl(pageUrl)
    pageHost = lookup ? new URL(lookup).host : ''
  } catch {}
  // Authorizing a page means its detected resources may reuse only cookies
  // that the browser would send to the resource URL itself. Page cookies are
  // never copied across origins.
  if (!config.useBrowserCookies && !config.authorizedCookieHosts.includes(host) && !config.authorizedCookieHosts.includes(pageHost)) return ''
  const values = await browser.cookies.getAll({ url: cookieUrl }).catch(() => [])
  return values.map(cookie => `${cookie.name}=${cookie.value}`).join('; ')
}

async function native(message: Record<string, unknown>, timeoutMs?: number): Promise<any> {
  if (!nativeBridge) return Promise.reject(new Error('Native Messaging 尚未初始化'))
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
  if (directBackend) {
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
  if (response?.bridge_base && response?.bridge_token) {
    directBackend = new BrowserDirectBackend(String(response.bridge_base), String(response.bridge_token))
  }
  return response
}

async function applyDesktopTakeoverSettings(response: any): Promise<any> {
  if (typeof response?.takeover_enabled === 'boolean' && Number.isFinite(Number(response?.takeover_minimum_bytes))) {
    await browser.storage.local.set({
      enabled: response.takeover_enabled,
      minimumBytes: Math.max(0, Number(response.takeover_minimum_bytes)),
    })
  }
  return response
}

async function pingDesktop(): Promise<any> {
  const response = await applyDesktopTakeoverSettings(await native({ op: 'ping' }))
  if (response?.ok) lastDesktopPingAt = Date.now()
  return response
}

async function inspectHls(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1): Promise<void> {
  const normalized = {
    ...resource,
    url: canonicalMediaUrl(resource.url, resource.kind),
  }
  const inspectionKey = `${tabId}:${normalized.pageUrl || ''}:${normalized.url}`
  if (normalized.kind !== 'hls' || !inspectedHls.claim(inspectionKey)) return
  try {
    const metadata = await inspectHlsResource(normalized)
    if (metadata) {
      const enriched = {
        ...normalized,
        ...metadata,
      }
      await saveResource(enriched, tabId)
      await sendCapturedResource(tabId, enriched)
    }
  } catch {
    // A transient CDN/auth failure must not permanently suppress inspection.
    // The captured manifest remains downloadable and the next observation can retry.
    inspectedHls.release(inspectionKey)
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

async function resourcePayload(resource: MediaResource, explicitChain?: RequestChain) {
  resource = { ...resource, url: canonicalMediaUrl(resource.url, resource.kind) }
  const pageUrl = await topLevelPageUrl(resource.tabId ?? -1, resource.pageUrl || '')
  const pageChain = resource.tabId !== undefined && resource.tabId >= 0
    ? requestChains.pageContext(resource.tabId, pageUrl)
    : undefined
  const requireSuccessfulRequest = resource.kind !== 'magnet'
  const chain = explicitChain || (resource.tabId !== undefined && resource.tabId >= 0
    ? requestChains.find({ url: resource.url, referrer: pageUrl }, Date.now(), resource.tabId, requireSuccessfulRequest)
    : requestChains.find({ url: resource.url, referrer: pageUrl }, Date.now(), undefined, requireSuccessfulRequest))
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
    ...replay,
    extension_version: extension.version,
    extension_client_id: await browserClientId(),
    extension_browser: extension.browser,
  }
}

async function downloadNow(resource: MediaResource, chain?: RequestChain) {
  const payload = await resourcePayload(resource, chain)
  return native({ op: 'download', resource: payload })
}

async function pushToTv(resource: MediaResource): Promise<{ ok: true }> {
  const response = await native({ op: 'media_push', kind: 'tvbox', resource: await resourcePayload(resource) })
  if (!response?.ok) throw new Error(response?.error || '电视推送失败')
  return { ok: true }
}

async function castToDevice(resource: MediaResource): Promise<{ ok: true }> {
  const response = await native({ op: 'media_push', kind: 'cast', resource: await resourcePayload(resource) })
  if (!response?.ok) throw new Error(response?.error || '投屏请求失败')
  return { ok: true }
}

async function offer(resource: MediaResource, chain?: RequestChain) {
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
    ...await resourcePayload(resource, chain),
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
      }, 2_000)
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

async function hydrateClickIntents(): Promise<void> {
  if (clickIntentsHydrated) return
  clickIntentsHydrated = true
  try {
    const stored = await browser.storage.session.get(CLICK_INTENT_STORAGE_KEY)
    const values = Array.isArray(stored[CLICK_INTENT_STORAGE_KEY]) ? stored[CLICK_INTENT_STORAGE_KEY] : []
    const now = Date.now()
    const restored = values
      .filter((item: any) => item && typeof item === 'object')
      .map((item: any) => ({
        href: String(item.href || ''),
        pageUrl: String(item.pageUrl || ''),
        altBypass: Boolean(item.altBypass),
        ctrlForce: Boolean(item.ctrlForce),
        generic: Boolean(item.generic),
        tabId: Number.isFinite(Number(item.tabId)) ? Number(item.tabId) : undefined,
        frameId: Number.isFinite(Number(item.frameId)) ? Number(item.frameId) : undefined,
        opensNewTab: Boolean(item.opensNewTab),
        controlHint: Boolean(item.controlHint),
        at: Number(item.at) || now,
      }))
      .filter((item: DownloadClickIntent) => now - item.at <= 7000)
    // Prefer fresher in-memory intents if both exist after a race.
    const merged = [...clickIntents, ...restored]
    const seen = new Set<string>()
    clickIntents = []
    for (const intent of merged) {
      const key = [intent.at, intent.href, intent.pageUrl, intent.tabId ?? '', intent.generic ? 1 : 0].join('|')
      if (seen.has(key)) continue
      seen.add(key)
      clickIntents.push(intent)
    }
    clickIntents = clickIntents
      .sort((left, right) => right.at - left.at)
      .slice(0, 20)
  } catch {
    // Session storage may be unavailable in rare test/host environments.
  }
}

async function persistClickIntents(): Promise<void> {
  const now = Date.now()
  clickIntents = clickIntents
    .filter(intent => now - intent.at <= 7000)
    .sort((left, right) => right.at - left.at)
    .slice(0, 20)
  try {
    await browser.storage.session.set({ [CLICK_INTENT_STORAGE_KEY]: clickIntents })
  } catch {
    // Ignore persistence failures; in-memory intents still work for the current worker.
  }
}

async function rememberClickIntent(intent: DownloadClickIntent): Promise<void> {
  await hydrateClickIntents()
  clickIntents.unshift(intent)
  await persistClickIntents()
  console.debug('HLS Downloader received explicit click intent', intent.href || intent.pageUrl || '')
}

async function consumeClickIntent(url: string, finalUrl = '', referrer = '', chain?: RequestChain): Promise<DownloadClickIntent | undefined> {
  await hydrateClickIntents()
  const now = Date.now()
  clickIntents = clickIntents.filter(intent => now - intent.at <= 7000)
  const index = clickIntents.findIndex(intent => matchesDownloadClick(intent, {
    url,
    finalUrl,
    referrer: referrer || chain?.pageUrl || '',
    chainUrls: chain?.urls,
    tabId: chain?.tabId,
  }, now))
  if (index < 0) {
    await persistClickIntents()
    return undefined
  }
  const [matched] = clickIntents.splice(index, 1)
  await persistClickIntents()
  return matched
}

async function waitForClickIntent(url: string, finalUrl = '', referrer = '', chain?: RequestChain): Promise<DownloadClickIntent | undefined> {
  // Downloads can lag behind click-intent by a few hundred ms on slow pages,
  // and Chrome may wake the service worker after the click message arrives.
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const intent = await consumeClickIntent(url, finalUrl, referrer, chain)
    if (intent) return intent
    await new Promise(resolve => setTimeout(resolve, 50))
  }
  return undefined
}

function consumeBrowserFallback(url: string): boolean {
  const now = Date.now()
  browserFallbacks = browserFallbacks.filter(item => now - item.at <= 7000)
  const index = browserFallbacks.findIndex(item => item.url === url)
  if (index < 0) return false
  browserFallbacks.splice(index, 1)
  return true
}

async function startBrowserFallback(url: string, filename = ''): Promise<number> {
  revealBrowserDownload()
  browserFallbacks.unshift({ url, at: Date.now() })
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
  const kind = disposition || mimeType.toLowerCase().includes('octet-stream')
    ? classifyDownload(details.url, mimeType, filename)
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
  }
  void saveResource(resource, details.tabId)
  void inspectHls(resource, details.tabId)
  void sendCapturedResource(details.tabId, resource)
  return { disposition, resource }
}

function trackedSize(chain: RequestChain | undefined): number {
  const contentRange = responseHeader(chain, 'content-range')
  const rangeTotal = Number(contentRange.match(/\/(\d+)$/)?.[1] || 0)
  return rangeTotal || Number(responseHeader(chain, 'content-length') || 0)
}

function isDownloadResponse(disposition: string, resource: { mimeType?: string, filename?: string } | null): boolean {
  if (!resource) return false
  return /(?:^|;)\s*attachment(?:;|$)/i.test(disposition)
    || Boolean(resource.filename)
    || resource.mimeType?.toLowerCase().includes('application/octet-stream') === true
}

export default defineBackground(() => {
  browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
    if (changeInfo.status === 'loading' || changeInfo.url) {
      requestChains.clearTab(tabId)
      inspectedHls.releasePrefix(`${tabId}:`)
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
  browser.tabs.onRemoved.addListener(tabId => {
    requestChains.clearTab(tabId)
    inspectedHls.releasePrefix(`${tabId}:`)
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
  void hydrateHandoffTracker().then(() => pollTrackedHandoffs()).catch(() => undefined)
  void setBrowserDownloadUi(true)
  void pingDesktop().catch(() => undefined)
  browser.alarms.create('desktop-heartbeat', { periodInMinutes: 0.5 })
  browser.alarms.create('handoff-tracker', { periodInMinutes: 0.5 })
  browser.alarms.onAlarm.addListener(alarm => {
    if (alarm.name === 'desktop-heartbeat') {
      requestChains.cleanup()
      void pingDesktop().catch(() => undefined)
    }
    if (alarm.name === 'handoff-tracker') void pollTrackedHandoffs()
  })
  browser.runtime.onInstalled.addListener(() => {
    // Updates keep old menu registrations. Rebuild them atomically so a
    // duplicate-id error cannot prevent the second item from being installed.
    void browser.contextMenus.removeAll().catch(() => undefined).then(() => {
      browser.contextMenus.create({ id: 'hls-download-link', title: '使用 HLS Downloader 下载', contexts: ['link', 'video', 'audio'] })
      browser.contextMenus.create({ id: 'hls-download-selection', title: '批量发送选中的链接', contexts: ['selection'] })
    })
  })

  ;(browser.webRequest.onSendHeaders.addListener as any)((details: any) => {
    requestChains.observeRequest(details)
  }, { urls: ['<all_urls>'] }, requestHeaderExtraInfo(import.meta.env.CHROME))
  ;(browser.webRequest.onBeforeRequest.addListener as any)((details: any) => {
    requestChains.observeRequest(details)
  }, { urls: ['<all_urls>'] }, ['requestBody'])
  browser.webRequest.onBeforeRedirect.addListener(details => {
    requestChains.observeRedirect(details as any)
  }, { urls: ['<all_urls>'] }, ['responseHeaders'])

  if (import.meta.env.FIREFOX) {
    ;(browser.webRequest.onHeadersReceived.addListener as any)(async (details: any) => {
      const chain = requestChains.observeResponse(details)
      const observed = observedResponse(details, chain)
      if (details.statusCode >= 300 && details.statusCode < 400) return {}
      if (!isDownloadResponse(observed.disposition, observed.resource)) return {}
      const intent = await resolveFirefoxClickIntent(
        undefined,
        () => waitForClickIntent(
          details.url,
          '',
          details.documentUrl || details.initiator || requestHeader(chain, 'referer'),
          chain,
        ),
      )
      if (!intent) return {}
      const config = await settings()
      const resource = observed.resource!
      resource.pageUrl = await topLevelPageUrl(details.tabId, resource.pageUrl)
      if (String(chain.method || '').toUpperCase() === 'POST' && !replayablePostRequest(chain).request_body) {
        // Never cancel a browser POST that cannot be reproduced exactly.
        return {}
      }
      if (!shouldTakeover({
        url: resource.url,
        size: resource.size,
        mimeType: resource.mimeType,
        filename: resource.filename,
        ...config,
        ...intent,
        explicitClick: true,
      }) || (!intent.ctrlForce && isHandoffSuppressed(config.suppressions, resource.pageUrl || '', resource.kind))) {
        return {}
      }
      try {
        const response = await offer({
          ...resource,
          requestHeaders: chain.requestHeaders,
          pageUrl: resource.pageUrl || chain.pageUrl || requestHeader(chain, 'referer'),
          id: resourceId(resource.url),
          seenAt: Date.now(),
        }, chain)
        const transferred = desktopAcceptedHandoff(response)
        return transferred ? { cancel: true } : {}
      } catch (error) {
        console.warn('HLS Downloader could not preempt Firefox response', error)
        return {}
      }
    }, { urls: ['<all_urls>'] }, ['blocking', 'responseHeaders'])
  } else {
    browser.webRequest.onHeadersReceived.addListener(details => {
      const chain = requestChains.observeResponse(details as any)
      observedResponse(details, chain)
      return undefined
    }, { urls: ['<all_urls>'] }, ['responseHeaders'])
  }
  browser.webRequest.onCompleted.addListener(details => {
    requestChains.finish(details.requestId, details.timeStamp || Date.now())
  }, { urls: ['<all_urls>'] })
  browser.webRequest.onErrorOccurred.addListener(details => {
    requestChains.fail(details.requestId)
  }, { urls: ['<all_urls>'] })

  filenameDeterminationEvent(import.meta.env.CHROME, browser.downloads as any)?.addListener((item: any, suggest: any) => {
    determinedDownloads.set(item.id, item)
    setTimeout(() => determinedDownloads.delete(item.id), 30_000)
    determinationWaiters.get(item.id)?.(item)
    suggest()
  })

  browser.downloads.onCreated.addListener(async item => {
    if (!item.url || item.url.startsWith('blob:')) return
    const creatingExtension = String((item as any).byExtensionId || '')
    if (creatingExtension && creatingExtension !== browser.runtime.id) return
    if (consumeBrowserFallback(item.url)) {
      revealBrowserDownload()
      return
    }
    console.debug('HLS Downloader observed browser download', item.url)
    try {
      const config = await settings()
      if (!config.enabled) return
      const actual = await refreshedDownload(item.id, item)
      if (!canContinueTakeover(actual.state)) return
      // Prefer the request chain first so click matching can use tabId even when
      // Chrome leaves DownloadItem.referrer empty. After a click is known, re-bind
      // the chain to that tab so we never replay another page's auth headers.
      const provisionalChain = requestChains.find(actual)
      let intent = await waitForClickIntent(
        actual.url,
        actual.finalUrl,
        actual.referrer || provisionalChain?.pageUrl || '',
        provisionalChain,
      )
      let chain = intent?.tabId === undefined
        ? provisionalChain
        : requestChains.find(actual, Date.now(), intent.tabId) || provisionalChain
      if (!intent) {
        intent = await waitForClickIntent(actual.url, actual.finalUrl, actual.referrer || '')
      }
      const url = chain?.finalUrl || actual.finalUrl || actual.url
      const contentDisposition = responseHeader(chain, 'content-disposition')
      const responseName = responseFilename(contentDisposition)
      const filename = responseName || actual.filename.split(/[\\/]/).pop() || ''
      const mimeType = actual.mime || responseHeader(chain, 'content-type')
      const kind = classifyDownload(url, mimeType, filename, contentDisposition)
      if (!kind) return
      const size = (actual.fileSize && actual.fileSize > 0 ? actual.fileSize : 0)
        || (actual.totalBytes && actual.totalBytes > 0 ? actual.totalBytes : 0)
        || trackedSize(chain)
      const pageUrl = actual.referrer || chain?.pageUrl || requestHeader(chain, 'referer')
      const sourcePageUrl = await topLevelPageUrl(chain?.tabId ?? -1, pageUrl)
      const resource: MediaResource = {
        id: resourceId(url), url, kind, mimeType, size, title: filename, filename,
        pageUrl: sourcePageUrl, tabId: chain?.tabId, method: chain?.method, requestHeaders: chain?.requestHeaders, seenAt: Date.now(),
      }
      if (String(chain?.method || '').toUpperCase() === 'POST' && !replayablePostRequest(chain).request_body) {
        return
      }
      if (!shouldTakeover({
        url: resource.url,
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
      console.debug('HLS Downloader offering verified browser download', url)
      const response = await offer(resource, chain)
      if (!desktopAcceptedHandoff(response)) throw new Error(response?.error || 'desktop rejected')
      // Do not discard the browser download just because the confirmation
      // window opened. The user owns the final decision; cancel/reject keeps
      // this original download in the browser.
      const handoff = await waitForHandoffResolution(String(response.handoff.id))
      if (handoff?.status !== 'accepted') return
      // Do not hide Chrome's downloads UI merely because a browser download was
      // observed. Suppress it only after the desktop accepted the handoff.
      concealBrowserDownload()
      await removeBrowserDownload(actual)
    } catch (error) {
      console.warn('HLS Downloader takeover failed; browser download remains untouched', error)
    } finally {
      determinedDownloads.delete(item.id)
      determinationWaiters.delete(item.id)
      revealBrowserDownload()
    }
  })

  browser.contextMenus.onClicked.addListener((info, tab) => {
    const url = info.linkUrl || info.srcUrl
    if (url) void offer({ id: resourceId(url), url, kind: classifyResource(url) || 'file', pageUrl: tab?.url, title: tab?.title, tabId: tab?.id, seenAt: Date.now() })
    if (info.menuItemId === 'hls-download-selection' && tab?.id) void browser.tabs.sendMessage(tab.id, { type: 'collect-selection' })
  })

  browser.runtime.onMessage.addListener((message, sender, sendResponse) => {
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
      void inspectHls(resource, sender.tab?.id ?? -1)
      return
    }
    if (message?.type === 'download' || message?.type === 'offer') {
      const resource = {
        ...message.resource,
        pageUrl: message.resource.pageUrl || sender.url || sender.tab?.url || '',
        tabId: message.resource.tabId ?? sender.tab?.id,
        frameId: message.resource.frameId ?? sender.frameId,
      }
      const request = message.type === 'offer' ? offer(resource) : downloadNow(resource)
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
      const key = storageKey(tabId, message.pageUrl)
      void browser.storage.session.get(key)
        .then(async value => {
          const raw = Array.isArray(value[key]) ? value[key] : []
          const cleaned = compactResources(raw, 40)
          if (cleaned.length !== raw.length) await browser.storage.session.set({ [key]: cleaned })
          if (tabId >= 0) await browser.action.setBadgeText({ tabId, text: cleaned.length ? String(Math.min(99, cleaned.length)) : '' })
          sendResponse(cleaned)
        })
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
      void native({
        op: 'set_takeover_settings',
        ...(typeof message.enabled === 'boolean' ? { enabled: message.enabled } : {}),
        ...(Number.isFinite(Number(message.minimumBytes)) ? { minimum_bytes: Number(message.minimumBytes) } : {}),
      }).then(applyDesktopTakeoverSettings)
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
