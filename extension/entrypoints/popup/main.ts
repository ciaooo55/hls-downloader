import { browser } from 'wxt/browser'
import type { MediaResource } from '../../lib/resources'
import { resourceQuality } from '../../lib/hlsManifest'
import { handoffStatusLabel, handoffTerminalStatus } from '../../lib/takeover'
import './style.css'

const root = document.getElementById('root')!

function formatDuration(seconds?: number) {
  if (!seconds || seconds <= 0) return ''
  const total = Math.round(seconds)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const mm = String(m).padStart(2, '0')
  const ss = String(s).padStart(2, '0')
  if (h) return h + ':' + mm + ':' + ss
  return m + ':' + ss
}

function formatSize(size: number) {
  const units = ['B', 'KB', 'MB', 'GB']
  let value = size
  let index = 0
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024
    index += 1
  }
  const amount = value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)
  return amount + ' ' + units[index]
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = '', text = '') {
  const node = document.createElement(tag)
  if (className) node.className = className
  if (text) node.textContent = text
  return node
}

async function main() {
  root.innerHTML = ''
  const mainEl = el('main')
  const header = el('header')
  const brand = el('div', 'brand')
  const logo = el('img') as HTMLImageElement
  logo.src = '/icon-32.png'
  logo.alt = ''
  logo.width = 18
  logo.height = 18
  const brandText = el('div')
  brandText.append(el('h1', '', 'HLS Downloader'), el('span', 'status', '\u8fde\u63a5\u4e2d\u2026'))
  brand.append(logo, brandText)
  const actions = el('div', 'header-actions')
  const openBtn = el('button', '', '\u6253\u5f00')
  openBtn.title = '\u6253\u5f00\u684c\u9762\u7aef'
  openBtn.addEventListener('click', () => void browser.runtime.sendMessage({ type: 'activate' }))
  const closeBtn = el('button', 'close-button', '\u00d7')
  closeBtn.title = '\u5173\u95ed'
  closeBtn.addEventListener('click', () => window.close())
  actions.append(openBtn, closeBtn)
  header.append(brand, actions)

  const controls = el('div', 'controls')
  const enableBtn = el('button', '', '\u81ea\u52a8\u63a5\u7ba1')
  const cookieBtn = el('button', '', 'Cookie')
  controls.append(enableBtn, cookieBtn)

  const errorBox = el('div', 'send-error')
  errorBox.hidden = true
  const section = el('section')
  const title = el('div', 'section-title', '\u5f53\u524d\u9875\u9762\u8d44\u6e90 ')
  const count = el('b', '', '0')
  title.append(count)
  const list = el('div', 'list')
  section.append(title, list)
  const footer = el('footer', '', 'Alt \u7ed5\u8fc7 \u00b7 Ctrl \u5f3a\u5236 \u00b7 \u8f7b\u91cf\u9762\u677f')
  mainEl.append(header, controls, errorBox, section, footer)
  root.append(mainEl)

  let enabled = true
  let host = ''
  let authorized: string[] = []
  const sending: Record<string, string> = {}
  const pending: Record<string, string> = {}
  let resources: MediaResource[] = []

  const statusEl = brandText.querySelector('.status') as HTMLSpanElement
  const setError = (message = '') => {
    errorBox.hidden = !message
    errorBox.textContent = message
  }

  const renderList = () => {
    count.textContent = String(resources.length)
    list.replaceChildren()
    if (!resources.length) {
      list.append(el('p', 'empty', '\u64ad\u653e\u5a92\u4f53\u540e\uff0c\u8fd9\u91cc\u4f1a\u663e\u793a\u53ef\u4e0b\u8f7d\u8d44\u6e90\u3002'))
      return
    }
    for (const item of resources) {
      let itemHost = item.url
      try { itemHost = new URL(item.url).host } catch {}
      const size = item.size && item.size > 0 ? formatSize(item.size) : '\u5927\u5c0f\u672a\u77e5'
      const quality = item.quality || resourceQuality(item.url, item.height)
      const resolution = item.width && item.height ? (item.width + '\u00d7' + item.height) : ''
      const bandwidth = item.bandwidth ? ((item.bandwidth / 1_000_000).toFixed(1) + ' Mbps') : ''
      const duration = item.duration ? formatDuration(item.duration) : ''
      const meta = [item.kind.toUpperCase(), quality, resolution, bandwidth, duration, size].filter(Boolean).join(' \u00b7 ')
      const article = el('article')
      const body = el('div')
      const name = el('strong', '', item.title || item.filename || item.url.split('/').pop() || item.url)
      name.title = item.filename || item.title || item.url
      const line = el('span', '', meta)
      const mime = el('small', '', [item.mimeType, itemHost].filter(Boolean).join(' \u00b7 '))
      const url = el('small', 'resource-url', item.url)
      url.title = item.url
      body.append(name, line, mime, url)
      const label = sending[item.id] || '\u4e0b\u8f7d'
      const button = el('button', '', label)
      const locked = ['\u53d1\u9001\u4e2d', '\u5f85\u786e\u8ba4', '\u786e\u8ba4\u4e2d', '\u5df2\u52a0\u5165'].includes(sending[item.id] || '')
      button.disabled = locked
      if (sending[item.id]) button.classList.add('busy')
      button.title = '\u53d1\u9001\u5230\u4e0b\u8f7d\u5668'
      button.addEventListener('click', () => void send(item))
      article.append(body, button)
      list.append(article)
    }
  }

  const refreshButtons = () => {
    enableBtn.textContent = enabled ? '\u81ea\u52a8\u63a5\u7ba1\u5f00' : '\u81ea\u52a8\u63a5\u7ba1\u5173'
    enableBtn.classList.toggle('active', enabled)
    cookieBtn.textContent = authorized.includes(host) ? 'Cookie \u5df2\u6388' : '\u6388\u6743 Cookie'
    cookieBtn.classList.toggle('active', authorized.includes(host))
    cookieBtn.disabled = !host
  }

  const send = async (item: MediaResource) => {
    setError('')
    sending[item.id] = '\u53d1\u9001\u4e2d'
    renderList()
    try {
      const response = await browser.runtime.sendMessage({ type: 'offer', resource: item })
      if (!response?.ok || !response?.handoff?.id) throw new Error(response?.error || '\u684c\u9762\u7aef\u6ca1\u6709\u521b\u5efa\u4e0b\u8f7d\u7a97\u53e3')
      sending[item.id] = '\u5f85\u786e\u8ba4'
      pending[item.id] = response.handoff.id
      renderList()
    } catch (reason) {
      sending[item.id] = '\u91cd\u8bd5'
      delete pending[item.id]
      setError(reason instanceof Error ? reason.message : '\u53d1\u9001\u5230\u684c\u9762\u7aef\u5931\u8d25')
      renderList()
    }
  }

  enableBtn.addEventListener('click', async () => {
    enabled = !enabled
    await browser.storage.local.set({ enabled })
    refreshButtons()
  })
  cookieBtn.addEventListener('click', async () => {
    if (!host) return
    authorized = authorized.includes(host) ? authorized.filter(value => value !== host) : [...authorized, host]
    await browser.storage.local.set({ authorizedCookieHosts: authorized })
    refreshButtons()
  })

  const [tab] = await browser.tabs.query({ active: true, currentWindow: true })
  const pageUrl = tab?.url || ''
  try { host = new URL(pageUrl).host } catch { host = '' }
  resources = await browser.runtime.sendMessage({ type: 'list', pageUrl, tabId: tab?.id }) || []
  const online = Boolean((await browser.runtime.sendMessage({ type: 'ping' }))?.ok)
  statusEl.textContent = online ? '\u684c\u9762\u7aef\u5df2\u8fde\u63a5' : '\u684c\u9762\u7aef\u79bb\u7ebf'
  statusEl.classList.toggle('online', online)
  const stored = await browser.storage.local.get(['enabled', 'authorizedCookieHosts'])
  enabled = stored.enabled !== false
  authorized = Array.isArray(stored.authorizedCookieHosts) ? stored.authorizedCookieHosts : []
  refreshButtons()
  renderList()

  window.setInterval(() => {
    const entries = Object.entries(pending)
    if (!entries.length) return
    void Promise.all(entries.map(async ([resourceId, handoffId]) => {
      try {
        const response = await browser.runtime.sendMessage({ type: 'handoff-status', handoffId })
        const handoff = response?.handoff || response
        const status = String(handoff?.status || '')
        if (!handoffTerminalStatus(status)) return
        sending[resourceId] = handoffStatusLabel(status)
        delete pending[resourceId]
        if (status === 'accepted') setError('')
        renderList()
      } catch {
        // Keep waiting while the native host briefly restarts.
      }
    }))
  }, 1200)
}

void main()
