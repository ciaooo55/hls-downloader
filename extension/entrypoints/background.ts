import { browser } from 'wxt/browser'
import { classifyDownload, classifyResource, mergeResources, resourceId, shouldTakeover, type MediaResource } from '../lib/resources'

const HOST = 'com.ciaooo55.hls_downloader'
const pending = new Map<number, string>()
let lastModifier = { altBypass: false, ctrlForce: false, at: 0 }

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

async function offer(resource: MediaResource) {
  return native({ op: 'offer', resource: await resourcePayload(resource) })
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

async function watchHandoff(downloadId: number, handoffId: string) {
  pending.set(downloadId, handoffId)
  for (let attempt = 0; attempt < 60; attempt += 1) {
    await new Promise(resolve => setTimeout(resolve, 1500))
    try {
      const response = await native({ op: 'handoff_status', handoff_id: handoffId })
      const status = response?.handoff?.status
      if (status === 'accepted') {
        await browser.downloads.cancel(downloadId).catch(() => undefined)
        await browser.downloads.erase({ id: downloadId }).catch(() => undefined)
        pending.delete(downloadId)
        return
      }
      if (status === 'rejected' || status === 'expired') break
    } catch { break }
  }
  await browser.downloads.resume(downloadId).catch(() => undefined)
  pending.delete(downloadId)
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
    const config = await settings()
    const url = item.finalUrl || item.url
    const filename = item.filename.split(/[\\/]/).pop() || ''
    const mimeType = item.mime || ''
    const kind = classifyDownload(url, mimeType, filename)
    if (!kind) return
    const resource: MediaResource = { id: resourceId(url), url, kind, mimeType, size: item.fileSize, title: filename, filename, pageUrl: item.referrer, seenAt: Date.now() }
    const modifiers = Date.now() - lastModifier.at < 2500 ? lastModifier : { altBypass: false, ctrlForce: false }
    if (!shouldTakeover({ url: resource.url, size: resource.size, mimeType, filename, ...config, ...modifiers })) return
    try {
      await pauseDownload(item.id)
      const response = await offer(resource)
      if (!response?.ok || !response?.handoff?.id) throw new Error(response?.error || 'desktop rejected')
      void watchHandoff(item.id, response.handoff.id)
    } catch {
      await browser.downloads.resume(item.id).catch(() => undefined)
    }
  })

  browser.contextMenus.onClicked.addListener((info, tab) => {
    const url = info.linkUrl || info.srcUrl
    if (url) void downloadNow({ id: resourceId(url), url, kind: classifyResource(url) || 'file', pageUrl: tab?.url, seenAt: Date.now() })
    if (info.menuItemId === 'hls-download-selection' && tab?.id) void browser.tabs.sendMessage(tab.id, { type: 'collect-selection' })
  })

  browser.runtime.onMessage.addListener((message, sender) => {
    if (message?.type === 'modifier') {
      lastModifier = { altBypass: Boolean(message.altKey), ctrlForce: Boolean(message.ctrlKey), at: Date.now() }
      return
    }
    if (message?.type === 'resource') {
      void saveResource({ ...message.resource, pageUrl: message.resource.pageUrl || sender.tab?.url }, sender.tab?.id ?? -1)
      return
    }
    if (message?.type === 'offer') return offer(message.resource)
    if (message?.type === 'download') return downloadNow(message.resource)
    if (message?.type === 'list') {
      const key = storageKey(Number(message.tabId ?? -1), message.pageUrl)
      return browser.storage.session.get(key).then(value => value[key] || [])
    }
    if (message?.type === 'ping') return native({ op: 'ping' }).catch(error => ({ ok: false, error: String(error) }))
  })
})
