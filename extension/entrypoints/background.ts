import { browser } from 'wxt/browser'
import { NativeBridge, type NativePortLike } from '../lib/nativeBridge'
import { classifyDownload, classifyResource, compactResources, matchesDownloadClick, mergeResources, pageResourceKey, replayableRequestHeaders, resourceId, resourceRequestIdentity, shouldTakeover, suggestedResourceFilename, type DownloadClickIntent, type MediaResource } from '../lib/resources'
import { RequestChainStore, requestHeader, responseHeader, type RequestChain } from '../lib/requestChain'
import { browserCleanupAction, canContinueTakeover, desktopAcceptedHandoff, handoffStatusLabel, handoffTerminalStatus, shouldResumeBrowserDownload } from '../lib/takeover'
import { filenameDeterminationEvent, requestHeaderExtraInfo, resolveFirefoxClickIntent } from '../lib/browserCapabilities'
import { parseHlsManifest, resourceQuality } from '../lib/hlsManifest'

const HOST = 'com.ciaooo55.hls_downloader'
let clickIntents: DownloadClickIntent[] = []
let browserFallbacks: Array<{ url: string, at: number }> = []
const determinedDownloads = new Map<number, Browser.downloads.DownloadItem>()
const determinationWaiters = new Map<number, (item: Browser.downloads.DownloadItem) => void>()
const requestChains = new RequestChainStore()
let nativeBridge: NativeBridge | null = null
let concealedDownloadCount = 0
let downloadUiFailsafe: ReturnType<typeof setTimeout> | null = null
const inspectedHls = new Set<string>()

async function settings() {
  const data = await browser.storage.local.get(['enabled', 'minimumBytes', 'excludedHosts', 'authorizedCookieHosts'])
  return {
    enabled: data.enabled !== false,
    minimumBytes: Number(data.minimumBytes ?? 1024 * 1024),
    excludedHosts: Array.isArray(data.excludedHosts) ? data.excludedHosts : [],
    authorizedCookieHosts: Array.isArray(data.authorizedCookieHosts) ? data.authorizedCookieHosts : [],
  }
}

function storageKey(tabId: number, pageUrl = '') {
  return pageResourceKey(tabId, pageUrl)
}

function responseFilename(value: string): string {
  const encoded = value.match(/filename\*=UTF-8''([^;]+)/i)?.[1] || ''
  if (encoded) {
    try { return decodeURIComponent(encoded).replace(/^"|"$/g, '') } catch { return encoded }
  }
  return value.match(/filename\s*=\s*"?([^";]+)/i)?.[1]?.trim() || ''
}

async function saveResource(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1) {
  const kind = resource.kind || classifyResource(resource.url, resource.mimeType)
  if (!kind) return
  let pageUrl = resource.pageUrl || ''
  if (tabId >= 0 && !pageUrl) {
    pageUrl = (await browser.tabs.get(tabId).catch(() => null))?.url || ''
  }
  const key = storageKey(tabId, pageUrl)
  const stored = await browser.storage.session.get(key)
  const resources = Array.isArray(stored[key]) ? stored[key] : []
  const merged = mergeResources(resources, { ...resource, pageUrl, kind, id: resourceId(resource.url), seenAt: Date.now() })
  await browser.storage.session.set({ [key]: merged })
  await browser.action.setBadgeText({ text: String(Math.min(99, merged.length)), ...(tabId >= 0 ? { tabId } : {}) })
}

async function cookiesFor(url: string, pageUrl = ''): Promise<string> {
  const config = await settings()
  const host = new URL(url).host
  let pageHost = ''
  try { pageHost = pageUrl ? new URL(pageUrl).host : '' } catch {}
  // Authorizing a page means its detected resources may reuse only cookies
  // that the browser would send to the resource URL itself. Page cookies are
  // never copied across origins.
  if (!config.authorizedCookieHosts.includes(host) && !config.authorizedCookieHosts.includes(pageHost)) return ''
  const values = await browser.cookies.getAll({ url })
  return values.map(cookie => `${cookie.name}=${cookie.value}`).join('; ')
}

function native(message: Record<string, unknown>, timeoutMs?: number): Promise<any> {
  if (!nativeBridge) return Promise.reject(new Error('Native Messaging 尚未初始化'))
  return nativeBridge.request(message, timeoutMs)
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
  return applyDesktopTakeoverSettings(await native({ op: 'ping', version: browser.runtime.getManifest().version }))
}

async function inspectHls(resource: Omit<MediaResource, 'id' | 'seenAt'>, tabId = -1): Promise<void> {
  const inspectionKey = `${tabId}:${resource.pageUrl || ''}:${resource.url}`
  if (resource.kind !== 'hls' || inspectedHls.has(inspectionKey)) return
  inspectedHls.add(inspectionKey)
  try {
    const response = await fetch(resource.url, { credentials: 'include', signal: AbortSignal.timeout(5_000) })
    if (!response.ok) return
    const info = parseHlsManifest(await response.text(), response.url || resource.url)
    if (info.duration || info.variants.length) {
      const variants = [...info.variants]
        .sort((left, right) => (right.height || 0) - (left.height || 0) || (right.bandwidth || 0) - (left.bandwidth || 0))
        .slice(0, 12)
      const best = variants[0]
      const enriched = {
        ...resource,
        duration: info.duration,
        variants,
        quality: best?.quality ? `最高 ${best.quality}` : resourceQuality(resource.url, resource.height),
      }
      await saveResource(enriched, tabId)
      if (tabId >= 0) await browser.tabs.sendMessage(tabId, { type: 'captured-resource', resource: enriched }).catch(() => undefined)
    }
  } catch {
    // Playlist inspection is best-effort; the captured URL remains downloadable.
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

async function resourcePayload(resource: MediaResource) {
  const pageUrl = resource.pageUrl || ''
  const identity = resourceRequestIdentity(resource, navigator.userAgent)
  const requestContexts: Record<string, Record<string, unknown>> = {}
  if (resource.tabId !== undefined && resource.tabId >= 0) {
    for (const chain of requestChains.contextsForPage(resource.tabId, pageUrl)) {
      let origin = ''
      try { origin = new URL(chain.finalUrl).origin } catch {}
      if (!origin) continue
      const scopedIdentity = resourceRequestIdentity({ pageUrl, requestHeaders: chain.requestHeaders })
      requestContexts[origin] = {
        request_headers: replayableRequestHeaders(chain.requestHeaders),
        referer: scopedIdentity.referer,
        origin: scopedIdentity.origin,
        user_agent: scopedIdentity.userAgent,
        cookie: await cookiesFor(chain.finalUrl, pageUrl),
      }
    }
  }
  return {
    url: resource.url,
    filename: suggestedResourceFilename(resource),
    title: resource.title || '',
    mime_type: resource.mimeType || '',
    size: resource.size || 0,
    source_page_url: pageUrl,
    referer: identity.referer,
    origin: identity.origin,
    cookie: await cookiesFor(resource.url, pageUrl),
    user_agent: identity.userAgent,
    request_headers: replayableRequestHeaders(resource.requestHeaders),
    request_contexts: requestContexts,
    extension_version: browser.runtime.getManifest().version,
  }
}

async function downloadNow(resource: MediaResource) {
  const payload = await resourcePayload(resource)
  return native({ op: 'download', resource: payload })
}

async function pushToTv(resource: MediaResource): Promise<{ ok: true }> {
  const data = await browser.storage.local.get(['tvboxEndpoint'])
  const endpoint = String(data.tvboxEndpoint || '').trim().replace(/\/+$/, '')
  if (!endpoint) throw new Error('请先在插件面板设置电视推送地址')
  try { new URL(endpoint) } catch { throw new Error('电视推送地址格式不正确') }
  if (!endpoint.startsWith('http://') && !endpoint.startsWith('https://')) {
    throw new Error('电视推送地址需以 http:// 或 https:// 开头')
  }
  const pushUrl = resource.url
  // TVBox standard push: POST /action with form body do=push&url=...
  // Some forks only accept GET; try POST first, fall back to GET on network error.
  try {
    const body = new URLSearchParams({ do: 'push', url: pushUrl })
    const response = await fetch(`${endpoint}/action`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: body.toString(),
      signal: AbortSignal.timeout(8_000),
    })
    if (response.ok) return { ok: true }
    // 404/405 means this fork doesn't accept POST; try GET
    if (response.status === 404 || response.status === 405) {
      const getResponse = await fetch(
        `${endpoint}/action?do=push&url=${encodeURIComponent(pushUrl)}`,
        { signal: AbortSignal.timeout(8_000) },
      )
      if (getResponse.ok) return { ok: true }
      throw new Error(`电视返回 HTTP ${getResponse.status}`)
    }
    throw new Error(`电视返回 HTTP ${response.status}`)
  } catch (error) {
    if (error instanceof TypeError) {
      throw new Error(`无法连接电视 (${endpoint})，请确认地址和局域网连通性`)
    }
    throw error
  }
}

async function offer(resource: MediaResource) {
  const payload = await resourcePayload(resource)
  const response = await native({ op: 'offer', resource: payload })
  const handoff = response?.handoff
  if (!response?.ok || !handoff?.id) return response

  // Desktop presentation is asynchronous. Wait briefly for the confirm UI so the
  // browser can still roll back if the window never appears.
  if (
    handoff.presentation_mode === 'desktop'
    || handoff.presentation_mode === 'desktop-pending'
    || handoff.presentation === 'queued'
  ) {
    const deadline = Date.now() + (handoff.presentation_mode === 'desktop-pending' ? 22_000 : 5_000)
    while (Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 180))
      try {
        const status = await native({ op: 'handoff_status', handoff_id: handoff.id }, 2_000)
        const current = status?.handoff || status
        if (current?.presentation === 'presented' || current?.presented) {
          return {
            ...response,
            handoff: {
              ...handoff,
              ...current,
              presentation_mode: current.presentation_mode || handoff.presentation_mode || 'desktop',
              presentation_ok: true,
            },
          }
        }
        if (current?.presentation === 'failed' || handoffTerminalStatus(current?.status)) {
          return {
            ok: false,
            error: current?.presentation_error || `接管请求${handoffStatusLabel(current?.status)}`,
            handoff: { ...handoff, ...current },
          }
        }
      } catch {
        // Keep waiting until the short presentation window expires.
      }
    }
    return { ok: false, error: '桌面端未能打开下载确认窗口', handoff }
  }
  return response
}

async function handoffStatus(handoffId: string) {
  return native({ op: 'handoff_status', handoff_id: handoffId }, 2_000)
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

async function pauseDownload(downloadId: number): Promise<boolean> {
  for (let attempt = 0; attempt < 12; attempt += 1) {
    try {
      await browser.downloads.pause(downloadId)
      return true
    } catch {
      await new Promise(resolve => setTimeout(resolve, 100))
    }
  }
  return false
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

function consumeClickIntent(url: string, finalUrl = '', referrer = '', chain?: RequestChain): DownloadClickIntent | undefined {
  const now = Date.now()
  clickIntents = clickIntents.filter(intent => now - intent.at <= 7000)
  const index = clickIntents.findIndex(intent => matchesDownloadClick(intent, {
    url,
    finalUrl,
    referrer: referrer || chain?.pageUrl || '',
    chainUrls: chain?.urls,
    tabId: chain?.tabId,
  }, now))
  if (index < 0) return undefined
  return clickIntents.splice(index, 1)[0]
}

async function waitForClickIntent(url: string, finalUrl = '', referrer = '', chain?: RequestChain): Promise<DownloadClickIntent | undefined> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    const intent = consumeClickIntent(url, finalUrl, referrer, chain)
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
  const kind = disposition
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
    requestHeaders: chain?.requestHeaders,
  }
  void saveResource(resource, details.tabId)
  void inspectHls(resource, details.tabId)
  if (details.tabId >= 0) void browser.tabs.sendMessage(details.tabId, { type: 'captured-resource', resource }).catch(() => undefined)
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
      void browser.action.setBadgeText({ tabId, text: '' }).catch(() => undefined)
    }
  })
  browser.tabs.onRemoved.addListener(tabId => {
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
  void setBrowserDownloadUi(true)
  void pingDesktop().catch(() => undefined)
  browser.alarms.create('desktop-heartbeat', { periodInMinutes: 0.5 })
  browser.alarms.onAlarm.addListener(alarm => {
    if (alarm.name === 'desktop-heartbeat') {
      requestChains.cleanup()
      void pingDesktop().catch(() => undefined)
    }
  })
  browser.runtime.onInstalled.addListener(() => {
    browser.contextMenus.create({ id: 'hls-download-link', title: '使用 HLS Downloader 下载', contexts: ['link', 'video', 'audio'] })
    browser.contextMenus.create({ id: 'hls-download-selection', title: '批量发送选中的链接', contexts: ['selection'] })
  })

  ;(browser.webRequest.onSendHeaders.addListener as any)((details: any) => {
    requestChains.observeRequest(details)
  }, { urls: ['<all_urls>'] }, requestHeaderExtraInfo(import.meta.env.CHROME))
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
      if (!shouldTakeover({
        url: resource.url,
        size: resource.size,
        mimeType: resource.mimeType,
        filename: resource.filename,
        ...config,
        ...intent,
        explicitClick: true,
      })) {
        return {}
      }
      try {
        const response = await offer({
          ...resource,
          requestHeaders: chain.requestHeaders,
          pageUrl: resource.pageUrl || chain.pageUrl || requestHeader(chain, 'referer'),
          id: resourceId(resource.url),
          seenAt: Date.now(),
        })
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

  filenameDeterminationEvent(import.meta.env.CHROME, browser.downloads as any)?.addListener((item: any, suggest: any) => {
    determinedDownloads.set(item.id, item)
    setTimeout(() => determinedDownloads.delete(item.id), 30_000)
    determinationWaiters.get(item.id)?.(item)
    suggest()
  })

  browser.downloads.onCreated.addListener(async item => {
    if (!item.url || item.url.startsWith('blob:')) return
    if (consumeBrowserFallback(item.url)) {
      revealBrowserDownload()
      return
    }
    console.debug('HLS Downloader observed browser download', item.url)
    concealBrowserDownload()
    let paused = false
    let handedOff = false
    try {
      const [pauseResult, config] = await Promise.all([pauseDownload(item.id), settings()])
      paused = pauseResult
      if (!config.enabled) {
        if (paused) await browser.downloads.resume(item.id).catch(() => undefined)
        return
      }
      const actual = await refreshedDownload(item.id, item)
      if (!canContinueTakeover(paused, actual.state)) return
      // Resolve the user's click before choosing a request chain. The same URL
      // can be active in several tabs; using the newest chain would otherwise
      // replay another page's Authorization or anti-bot headers.
      let intent = await waitForClickIntent(actual.url, actual.finalUrl, actual.referrer || '')
      let chain = requestChains.find(actual, Date.now(), intent?.tabId)
      if (!intent) {
        const provisionalChain = requestChains.find(actual)
        intent = await waitForClickIntent(
          actual.url,
          actual.finalUrl,
          actual.referrer || provisionalChain?.pageUrl || '',
          provisionalChain,
        )
        chain = intent?.tabId === undefined
          ? provisionalChain
          : requestChains.find(actual, Date.now(), intent.tabId)
      }
      const url = chain?.finalUrl || actual.finalUrl || actual.url
      const responseName = responseFilename(responseHeader(chain, 'content-disposition'))
      const filename = responseName || actual.filename.split(/[\\/]/).pop() || ''
      const mimeType = actual.mime || responseHeader(chain, 'content-type')
      const kind = classifyDownload(url, mimeType, filename) || 'file'
      const size = (actual.fileSize && actual.fileSize > 0 ? actual.fileSize : 0)
        || (actual.totalBytes && actual.totalBytes > 0 ? actual.totalBytes : 0)
        || trackedSize(chain)
      const pageUrl = actual.referrer || chain?.pageUrl || requestHeader(chain, 'referer')
      const resource: MediaResource = {
        id: resourceId(url), url, kind, mimeType, size, title: filename, filename,
        pageUrl, tabId: chain?.tabId, requestHeaders: chain?.requestHeaders, seenAt: Date.now(),
      }
      if (!intent || !shouldTakeover({ url: resource.url, size: resource.size, mimeType, filename, ...config, ...intent, explicitClick: true })) {
        await browser.downloads.resume(item.id).catch(() => undefined)
        return
      }
      console.debug('HLS Downloader taking over explicit browser download', url)
      const response = await offer(resource)
      if (!desktopAcceptedHandoff(response)) throw new Error(response?.error || 'desktop rejected')
      handedOff = true
      await removeBrowserDownload(actual)
    } catch (error) {
      console.warn('HLS Downloader takeover failed; returning download to browser', error)
      if (shouldResumeBrowserDownload(paused, handedOff)) await browser.downloads.resume(item.id).catch(() => undefined)
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
      clickIntents.unshift({
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
      clickIntents = clickIntents.slice(0, 20)
      console.debug('HLS Downloader received explicit click intent', message.href || sender.tab?.url || '')
      return
    }
    if (message?.type === 'resource') {
      const resource = { ...message.resource, pageUrl: message.resource.pageUrl || sender.tab?.url }
      void saveResource(resource, sender.tab?.id ?? -1)
      void inspectHls(resource, sender.tab?.id ?? -1)
      return
    }
    if (message?.type === 'download' || message?.type === 'offer') {
      const resource = {
        ...message.resource,
        pageUrl: message.resource.pageUrl || sender.tab?.url || '',
        tabId: message.resource.tabId ?? sender.tab?.id,
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
        .catch(error => sendResponse({ ok: false, error: String(error) }))
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
    if (message?.type === 'activate') {
      void native({ op: 'activate' })
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
  })
})
