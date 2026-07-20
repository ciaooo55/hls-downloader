import { browser } from 'wxt/browser'
import { classifyDownload, classifyResource, matchesDownloadClick, mergeResources, resourceId, shouldTakeover, type DownloadClickIntent, type MediaResource } from '../lib/resources'
import { browserCleanupAction, shouldResumeBrowserDownload } from '../lib/takeover'

const HOST = 'com.ciaooo55.hls_downloader'
let clickIntents: DownloadClickIntent[] = []
let browserFallbacks: Array<{ url: string, at: number }> = []
const firefoxRequestIntents = new Map<string, DownloadClickIntent>()

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

function native(message: Record<string, unknown>): Promise<any> {
  return browser.runtime.sendNativeMessage(HOST, message)
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
  await new Promise(resolve => setTimeout(resolve, 150))
  const [current] = await browser.downloads.search({ id: downloadId })
  return current || original
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

function consumeClickIntent(url: string, finalUrl = ''): DownloadClickIntent | undefined {
  const now = Date.now()
  clickIntents = clickIntents.filter(intent => now - intent.at <= 7000)
  const index = clickIntents.findIndex(intent => matchesDownloadClick(intent, { url, finalUrl }, now))
  if (index < 0) return undefined
  return clickIntents.splice(index, 1)[0]
}

async function waitForClickIntent(url: string, finalUrl = ''): Promise<DownloadClickIntent | undefined> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    const intent = consumeClickIntent(url, finalUrl)
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
  void native({ op: 'ping', version: browser.runtime.getManifest().version }).catch(() => undefined)
  browser.runtime.onInstalled.addListener(() => {
    browser.contextMenus.create({ id: 'hls-download-link', title: '使用 HLS Downloader 下载', contexts: ['link', 'video', 'audio'] })
    browser.contextMenus.create({ id: 'hls-download-selection', title: '批量发送选中的链接', contexts: ['selection'] })
  })

  if (import.meta.env.FIREFOX) {
    ;(browser.webRequest.onBeforeRequest.addListener as any)((details: any) => {
      if (firefoxRequestIntents.has(details.requestId)) return
      const intent = consumeClickIntent(details.url)
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
        if (!response?.ok || !response?.handoff?.id) return {}
        firefoxRequestIntents.delete(details.requestId)
        return { cancel: true }
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

  browser.downloads.onCreated.addListener(async item => {
    if (!item.url || item.url.startsWith('blob:')) return
    if (consumeBrowserFallback(item.url)) return
    console.debug('HLS Downloader observed browser download', item.url)
    const config = await settings()
    if (!config.enabled) return
    let paused = false
    let handedOff = false
    try {
      paused = await pauseDownload(item.id)
      const actual = await refreshedDownload(item.id, item)
      const url = actual.finalUrl || actual.url
      const filename = actual.filename.split(/[\\/]/).pop() || ''
      const mimeType = actual.mime || ''
      const kind = classifyDownload(url, mimeType, filename) || 'file'
      const resource: MediaResource = { id: resourceId(url), url, kind, mimeType, size: actual.fileSize || actual.totalBytes, title: filename, filename, pageUrl: actual.referrer, seenAt: Date.now() }
      const intent = await waitForClickIntent(actual.url, actual.finalUrl)
      if (!intent || !shouldTakeover({ url: resource.url, size: resource.size, mimeType, filename, ...config, ...intent, explicitClick: true })) {
        await browser.downloads.resume(item.id).catch(() => undefined)
        return
      }
      console.debug('HLS Downloader taking over explicit browser download', url)
      const response = await offer(resource)
      const handoffId = String(response?.handoff?.id || '')
      if (!response?.ok || !handoffId) throw new Error(response?.error || 'desktop rejected')
      handedOff = true
      await removeBrowserDownload(actual)
    } catch (error) {
      console.warn('HLS Downloader takeover failed; returning download to browser', error)
      if (shouldResumeBrowserDownload(paused, handedOff)) await browser.downloads.resume(item.id).catch(() => undefined)
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
    if (message?.type === 'preempt-download') {
      void (async () => {
        const resource = message.resource as MediaResource
        const intent = message.intent as DownloadClickIntent
        const config = await settings()
        const takeover = shouldTakeover({
          url: resource.url,
          size: resource.size,
          mimeType: resource.mimeType,
          filename: resource.filename,
          ...config,
          ...intent,
          explicitClick: true,
        })
        if (!takeover) {
          const downloadId = await startBrowserFallback(resource.url, resource.filename)
          return { ok: false, fallback: true, downloadId }
        }
        try {
          const response = await offer(resource)
          if (!response?.ok || !response?.handoff?.id) throw new Error(response?.error || 'desktop rejected')
          return response
        } catch (error) {
          console.warn('HLS Downloader pre-click takeover failed; returning link to browser', error)
          const downloadId = await startBrowserFallback(resource.url, resource.filename)
          return { ok: false, fallback: true, downloadId, error: String(error) }
        }
      })().then(sendResponse).catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
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
  })
})
