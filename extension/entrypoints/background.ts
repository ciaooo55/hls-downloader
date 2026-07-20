import { browser } from 'wxt/browser'
import { NativeBridge, type NativePortLike } from '../lib/nativeBridge'
import { classifyDownload, classifyResource, matchesDownloadClick, mergeResources, resourceId, shouldTakeover, type DownloadClickIntent, type MediaResource } from '../lib/resources'
import { browserCleanupAction, handoffOutcome, shouldResumeBrowserDownload } from '../lib/takeover'

const HOST = 'com.ciaooo55.hls_downloader'
let clickIntents: DownloadClickIntent[] = []
let browserFallbacks: Array<{ url: string, at: number }> = []
const firefoxRequestIntents = new Map<string, DownloadClickIntent>()
const determinedDownloads = new Map<number, browser.downloads.DownloadItem>()
const determinationWaiters = new Map<number, (item: browser.downloads.DownloadItem) => void>()
let nativeBridge: NativeBridge | null = null
let concealedDownloadCount = 0
let downloadUiFailsafe: ReturnType<typeof setTimeout> | null = null

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
  return tabId >= 0 ? `resources:tab:${tabId}` : `resources:${pageUrl || 'global'}`
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
  const key = storageKey(tabId, resource.pageUrl)
  const stored = await browser.storage.session.get(key)
  const resources = Array.isArray(stored[key]) ? stored[key] : []
  const merged = mergeResources(resources, { ...resource, kind, id: resourceId(resource.url), seenAt: Date.now() })
  await browser.storage.session.set({ [key]: merged })
  await browser.action.setBadgeText({ text: String(Math.min(99, merged.length)), ...(tabId >= 0 ? { tabId } : {}) })
}

async function cookiesFor(url: string, pageUrl = ''): Promise<string> {
  const config = await settings()
  const host = new URL(url).host
  if (!config.authorizedCookieHosts.includes(host)) return ''
  const values = await browser.cookies.getAll({ url })
  return values.map(cookie => `${cookie.name}=${cookie.value}`).join('; ')
}

function native(message: Record<string, unknown>, timeoutMs?: number): Promise<any> {
  if (!nativeBridge) return Promise.reject(new Error('Native Messaging 尚未初始化'))
  return nativeBridge.request(message, timeoutMs)
}

async function waitForHandoff(handoffId: string): Promise<any> {
  return native({
    op: 'wait_handoff',
    handoff_id: handoffId,
    version: browser.runtime.getManifest().version,
  }, 130_000)
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
  let pageOrigin = ''
  try { pageOrigin = pageUrl ? new URL(pageUrl).origin : '' } catch {}
  return {
    url: resource.url,
    filename: resource.filename || resource.title || '',
    mime_type: resource.mimeType || '',
    size: resource.size || 0,
    source_page_url: pageUrl,
    referer: pageUrl,
    origin: pageOrigin,
    cookie: await cookiesFor(resource.url, pageUrl),
    user_agent: navigator.userAgent,
    extension_version: browser.runtime.getManifest().version,
  }
}

async function downloadNow(resource: MediaResource) {
  const payload = await resourcePayload(resource)
  return native({ op: 'download', resource: payload })
}

async function offer(resource: MediaResource) {
  const payload = await resourcePayload(resource)
  return native({ op: 'offer', resource: payload })
}

async function refreshedDownload(downloadId: number, original: browser.downloads.DownloadItem) {
  let determined = determinedDownloads.get(downloadId)
  if (!determined) {
    determined = await new Promise<browser.downloads.DownloadItem | undefined>(resolve => {
      const timeout = setTimeout(() => {
        determinationWaiters.delete(downloadId)
        resolve(undefined)
      }, 500)
      determinationWaiters.set(downloadId, item => {
        clearTimeout(timeout)
        determinationWaiters.delete(downloadId)
        resolve(item)
      })
    })
  }
  const [current] = await browser.downloads.search({ id: downloadId })
  return determined || current || original
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

async function removeBrowserDownload(item: browser.downloads.DownloadItem): Promise<void> {
  const [current] = await browser.downloads.search({ id: item.id })
  const state = current?.state || item.state
  if (browserCleanupAction(state) === 'remove-file') {
    await browser.downloads.removeFile(item.id).catch(() => undefined)
  } else {
    await browser.downloads.cancel(item.id).catch(() => undefined)
  }
  await browser.downloads.erase({ id: item.id }).catch(() => undefined)
}

function consumeClickIntent(url: string, finalUrl = '', referrer = ''): DownloadClickIntent | undefined {
  const now = Date.now()
  clickIntents = clickIntents.filter(intent => now - intent.at <= 7000)
  const index = clickIntents.findIndex(intent => matchesDownloadClick(intent, { url, finalUrl, referrer }, now))
  if (index < 0) return undefined
  return clickIntents.splice(index, 1)[0]
}

async function waitForClickIntent(url: string, finalUrl = '', referrer = ''): Promise<DownloadClickIntent | undefined> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    const intent = consumeClickIntent(url, finalUrl, referrer)
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

function observedResponse(details: any) {
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
    pageUrl: details.documentUrl || details.initiator || '',
  }
  void saveResource(resource, details.tabId)
  if (details.tabId >= 0) void browser.tabs.sendMessage(details.tabId, { type: 'captured-resource', resource }).catch(() => undefined)
  return { disposition, resource }
}

function isDownloadResponse(disposition: string, resource: { mimeType?: string, filename?: string } | null): boolean {
  if (!resource) return false
  return /(?:^|;)\s*attachment(?:;|$)/i.test(disposition)
    || Boolean(resource.filename)
    || resource.mimeType?.toLowerCase().includes('application/octet-stream') === true
}

export default defineBackground(() => {
  nativeBridge = new NativeBridge(
    () => browser.runtime.connectNative(HOST) as unknown as NativePortLike,
    30_000,
    () => {
      concealedDownloadCount = 0
      revealBrowserDownload()
    },
  )
  void setBrowserDownloadUi(true)
  void native({ op: 'ping', version: browser.runtime.getManifest().version }).catch(() => undefined)
  browser.runtime.onInstalled.addListener(() => {
    browser.contextMenus.create({ id: 'hls-download-link', title: '使用 HLS Downloader 下载', contexts: ['link', 'video', 'audio'] })
    browser.contextMenus.create({ id: 'hls-download-selection', title: '批量发送选中的链接', contexts: ['selection'] })
  })

  if (import.meta.env.FIREFOX) {
    ;(browser.webRequest.onBeforeRequest.addListener as any)((details: any) => {
      if (firefoxRequestIntents.has(details.requestId)) return
      const intent = consumeClickIntent(details.url, '', details.documentUrl || details.initiator || '')
      if (intent) firefoxRequestIntents.set(details.requestId, intent)
    }, { urls: ['<all_urls>'] })
    ;(browser.webRequest.onHeadersReceived.addListener as any)(async (details: any) => {
      const observed = observedResponse(details)
      const intent = firefoxRequestIntents.get(details.requestId)
      if (!intent || details.statusCode >= 300 && details.statusCode < 400) return {}
      if (!isDownloadResponse(observed.disposition, observed.resource)) {
        firefoxRequestIntents.delete(details.requestId)
        return {}
      }
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
        firefoxRequestIntents.delete(details.requestId)
        return {}
      }
      try {
        const response = await offer({ ...resource, id: resourceId(resource.url), seenAt: Date.now() })
        const handoffId = String(response?.handoff?.id || '')
        if (!response?.ok || !handoffId) return {}
        const decision = await waitForHandoff(handoffId)
        const outcome = handoffOutcome(String(decision?.handoff?.status || ''))
        firefoxRequestIntents.delete(details.requestId)
        return outcome === 'desktop' || outcome === 'cancel' ? { cancel: true } : {}
      } catch (error) {
        console.warn('HLS Downloader could not preempt Firefox response', error)
        firefoxRequestIntents.delete(details.requestId)
        return {}
      }
    }, { urls: ['<all_urls>'] }, ['blocking', 'responseHeaders'])
    const clearFirefoxIntent = (details: any) => firefoxRequestIntents.delete(details.requestId)
    browser.webRequest.onCompleted.addListener(clearFirefoxIntent, { urls: ['<all_urls>'] })
    browser.webRequest.onErrorOccurred.addListener(clearFirefoxIntent, { urls: ['<all_urls>'] })
  } else {
    browser.webRequest.onHeadersReceived.addListener(details => {
      observedResponse(details)
    }, { urls: ['<all_urls>'] }, ['responseHeaders'])
  }

  browser.downloads.onDeterminingFilename.addListener((item, suggest) => {
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
      if (!paused || !config.enabled) {
        if (paused) await browser.downloads.resume(item.id).catch(() => undefined)
        return
      }
      const actual = await refreshedDownload(item.id, item)
      const url = actual.finalUrl || actual.url
      const filename = actual.filename.split(/[\\/]/).pop() || ''
      const mimeType = actual.mime || ''
      const kind = classifyDownload(url, mimeType, filename) || 'file'
      const resource: MediaResource = { id: resourceId(url), url, kind, mimeType, size: actual.fileSize || actual.totalBytes, title: filename, filename, pageUrl: actual.referrer, seenAt: Date.now() }
      const intent = await waitForClickIntent(actual.url, actual.finalUrl, actual.referrer)
      if (!intent || !shouldTakeover({ url: resource.url, size: resource.size, mimeType, filename, ...config, ...intent, explicitClick: true })) {
        await browser.downloads.resume(item.id).catch(() => undefined)
        return
      }
      console.debug('HLS Downloader taking over explicit browser download', url)
      const response = await offer(resource)
      const handoffId = String(response?.handoff?.id || '')
      if (!response?.ok || !handoffId) throw new Error(response?.error || 'desktop rejected')
      const decision = await waitForHandoff(handoffId)
      const status = String(decision?.handoff?.status || '')
      const outcome = handoffOutcome(status)
      if (outcome === 'browser') throw new Error(`desktop handoff ended with ${status || 'unknown status'}`)
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
    if (url) void offer({ id: resourceId(url), url, kind: classifyResource(url) || 'file', pageUrl: tab?.url, seenAt: Date.now() })
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
        at: Date.now(),
      })
      clickIntents = clickIntents.slice(0, 20)
      console.debug('HLS Downloader received explicit click intent', message.href || sender.tab?.url || '')
      return
    }
    if (message?.type === 'resource') {
      void saveResource({ ...message.resource, pageUrl: message.resource.pageUrl || sender.tab?.url }, sender.tab?.id ?? -1)
      return
    }
    if (message?.type === 'download' || message?.type === 'offer') {
      const request = message.type === 'offer' ? offer(message.resource) : downloadNow(message.resource)
      void request
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
      const key = storageKey(Number(message.tabId ?? -1), message.pageUrl)
      void browser.storage.session.get(key)
        .then(value => sendResponse(value[key] || []))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'ping') {
      void native({ op: 'ping' })
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
