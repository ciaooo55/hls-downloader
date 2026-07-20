import { browser } from 'wxt/browser'
import { classifyDownload, classifyResource, matchesDownloadClick, mergeResources, resourceId, shouldTakeover, type DownloadClickIntent, type MediaResource } from '../lib/resources'

const HOST = 'com.ciaooo55.hls_downloader'
let clickIntents: DownloadClickIntent[] = []
let browserFallbacks: Array<{ url: string, at: number }> = []

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

async function pauseDownload(downloadId: number): Promise<void> {
  for (let attempt = 0; attempt < 12; attempt += 1) {
    try {
      await browser.downloads.pause(downloadId)
      return
    } catch {
      await new Promise(resolve => setTimeout(resolve, 100))
    }
  }
  throw new Error('浏览器下载无法暂停')
}

function consumeClickIntent(url: string, referrer = ''): DownloadClickIntent | undefined {
  const now = Date.now()
  clickIntents = clickIntents.filter(intent => now - intent.at <= 7000)
  const index = clickIntents.findIndex(intent => matchesDownloadClick(intent, { url, referrer }, now))
  if (index < 0) return undefined
  return clickIntents.splice(index, 1)[0]
}

async function waitForClickIntent(url: string, referrer = ''): Promise<DownloadClickIntent | undefined> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    const intent = consumeClickIntent(url, referrer)
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

export default defineBackground(() => {
  void native({ op: 'ping', version: browser.runtime.getManifest().version }).catch(() => undefined)
  browser.runtime.onInstalled.addListener(() => {
    browser.contextMenus.create({ id: 'hls-download-link', title: '使用 HLS Downloader 下载', contexts: ['link', 'video', 'audio'] })
    browser.contextMenus.create({ id: 'hls-download-selection', title: '批量发送选中的链接', contexts: ['selection'] })
  })

  browser.webRequest.onHeadersReceived.addListener(details => {
    const headers = details.responseHeaders || []
    const mimeType = headers.find(header => header.name?.toLowerCase() === 'content-type')?.value || ''
    const contentRange = headers.find(header => header.name?.toLowerCase() === 'content-range')?.value || ''
    const rangeTotal = Number(contentRange.match(/\/(\d+)$/)?.[1] || 0)
    const length = rangeTotal || Number(headers.find(header => header.name?.toLowerCase() === 'content-length')?.value || 0)
    const disposition = headers.find(header => header.name?.toLowerCase() === 'content-disposition')?.value || ''
    const filename = responseFilename(disposition)
    const kind = disposition
      ? classifyDownload(details.url, mimeType, filename)
      : classifyResource(details.url, mimeType)
    if (kind) {
      const resource = { url: details.url, kind, mimeType, size: length, filename, statusCode: details.statusCode, method: details.method, pageUrl: details.documentUrl || details.initiator || '' }
      void saveResource(resource, details.tabId)
      if (details.tabId >= 0) void browser.tabs.sendMessage(details.tabId, { type: 'captured-resource', resource }).catch(() => undefined)
    }
  }, { urls: ['<all_urls>'] }, ['responseHeaders'])

  browser.downloads.onCreated.addListener(async item => {
    if (!item.url || item.url.startsWith('blob:')) return
    if (consumeBrowserFallback(item.url)) return
    console.debug('HLS Downloader observed browser download', item.url)
    const config = await settings()
    if (!config.enabled) return
    const url = item.finalUrl || item.url
    const filename = item.filename.split(/[\\/]/).pop() || ''
    const mimeType = item.mime || ''
    const kind = classifyDownload(url, mimeType, filename) || 'file'
    const resource: MediaResource = { id: resourceId(url), url, kind, mimeType, size: item.fileSize, title: filename, filename, pageUrl: item.referrer, seenAt: Date.now() }
    let paused = false
    try {
      await pauseDownload(item.id)
      paused = true
      const intent = await waitForClickIntent(url, item.referrer)
      if (!intent || !shouldTakeover({ url: resource.url, size: resource.size, mimeType, filename, ...config, ...intent, explicitClick: true })) {
        await browser.downloads.resume(item.id).catch(() => undefined)
        return
      }
      console.debug('HLS Downloader taking over explicit browser download', url)
      const response = await downloadNow(resource)
      if (!response?.ok || !response?.task?.id) throw new Error(response?.error || 'desktop rejected')
      await browser.downloads.cancel(item.id)
      await browser.downloads.erase({ id: item.id }).catch(() => undefined)
    } catch (error) {
      console.warn('HLS Downloader takeover failed; returning download to browser', error)
      if (paused) await browser.downloads.resume(item.id).catch(() => undefined)
    }
  })

  browser.contextMenus.onClicked.addListener((info, tab) => {
    const url = info.linkUrl || info.srcUrl
    if (url) void downloadNow({ id: resourceId(url), url, kind: classifyResource(url) || 'file', pageUrl: tab?.url, seenAt: Date.now() })
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
    if (message?.type === 'download' || message?.type === 'direct-click-download') {
      const request = message.type === 'direct-click-download'
        ? settings().then(config => shouldTakeover({
            url: String(message.resource?.url || ''), ...config,
            explicitClick: true, altBypass: Boolean(message.altBypass), ctrlForce: Boolean(message.ctrlForce),
          }) ? downloadNow(message.resource) : ({ ok: false, bypass: true }))
        : downloadNow(message.resource)
      void request
        .then(response => sendResponse(response))
        .catch(error => sendResponse({ ok: false, error: String(error) }))
      return true
    }
    if (message?.type === 'browser-download') {
      const url = String(message.url || '')
      browserFallbacks.unshift({ url, at: Date.now() })
      void browser.downloads.download({
        url,
        ...(message.filename ? { filename: String(message.filename) } : {}),
      }).then(downloadId => sendResponse({ ok: true, downloadId })).catch(error => {
        consumeBrowserFallback(url)
        sendResponse({ ok: false, error: String(error) })
      })
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
