import { browser } from 'wxt/browser'
import { visibleMediaResources, type MediaResource } from '../../lib/resources'
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
  root.replaceChildren()
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
  const excludeBtn = el('button', '', '\u6392\u9664\u672c\u7ad9')
  controls.append(enableBtn, cookieBtn, excludeBtn)

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
  let resourceHosts: string[] = []
  let authorized: string[] = []
  let excluded: string[] = []
  const sending: Record<string, string> = {}
  const pending: Record<string, string> = {}
  let resources: MediaResource[] = []

  const statusEl = brandText.querySelector('.status') as HTMLSpanElement
  const setError = (message = '') => {
    errorBox.hidden = !message
    errorBox.textContent = message
  }

  const renderList = () => {
    const visible = visibleMediaResources(resources)
    count.textContent = String(visible.length)
    list.replaceChildren()
    if (!visible.length) {
      list.append(el('p', 'empty', '\u64ad\u653e\u5a92\u4f53\u540e\uff0c\u8fd9\u91cc\u4f1a\u663e\u793a\u53ef\u4e0b\u8f7d\u8d44\u6e90\u3002'))
      return
    }
    for (const item of visible) {
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
      let selected = item
      body.append(name, line)
      if (item.variants?.length) {
        const select = el('select', 'quality-select') as HTMLSelectElement
        select.setAttribute('aria-label', '\u9009\u62e9\u89c6\u9891\u6e05\u6670\u5ea6')
        const automatic = el('option', '', '\u81ea\u52a8\uff08\u6700\u9ad8\uff09') as HTMLOptionElement
        automatic.value = item.url
        select.append(automatic)
        for (const variant of item.variants) {
          const option = el('option') as HTMLOptionElement
          option.value = variant.url
          option.textContent = [variant.quality || (variant.height ? `${variant.height}p` : '\u7ebf\u8def'), variant.bandwidth ? `${(variant.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''].filter(Boolean).join(' \u00b7 ')
          select.append(option)
        }
        select.addEventListener('change', () => {
          const variant = item.variants?.find(value => value.url === select.value)
          selected = variant ? { ...item, ...variant, url: variant.url, variants: undefined } : item
        })
        body.append(select)
      }
      body.append(mime, url)
      const label = sending[item.id] || '\u4e0b\u8f7d'
      const button = el('button', '', label)
      const locked = ['\u53d1\u9001\u4e2d', '\u5f85\u786e\u8ba4', '\u786e\u8ba4\u4e2d', '\u5df2\u52a0\u5165'].includes(sending[item.id] || '')
      button.disabled = locked
      if (sending[item.id]) button.classList.add('busy')
      button.title = '\u53d1\u9001\u5230\u4e0b\u8f7d\u5668'
      button.addEventListener('click', () => void send(selected))
      article.append(body, button)
      list.append(article)
    }
  }

  const refreshButtons = () => {
    enableBtn.textContent = enabled ? '\u81ea\u52a8\u63a5\u7ba1\u5f00' : '\u81ea\u52a8\u63a5\u7ba1\u5173'
    enableBtn.classList.toggle('active', enabled)
    const consentHosts = [...new Set([host, ...resourceHosts].filter(Boolean))]
    const allAuthorized = Boolean(consentHosts.length) && consentHosts.every(value => authorized.includes(value))
    cookieBtn.textContent = allAuthorized ? `Cookie \u5df2\u6388 (${consentHosts.length})` : '\u6388\u6743\u672c\u9875 Cookie'
    cookieBtn.classList.toggle('active', allAuthorized)
    cookieBtn.disabled = !host
    const siteExcluded = excluded.includes(host)
    excludeBtn.textContent = siteExcluded ? '\u672c\u7ad9\u5df2\u6392\u9664' : '\u6392\u9664\u672c\u7ad9'
    excludeBtn.classList.toggle('active', siteExcluded)
    excludeBtn.disabled = !host
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
    const requested = !enabled
    const response = await browser.runtime.sendMessage({ type: 'set-takeover-settings', enabled: requested })
    if (!response?.ok) {
      setError(response?.error || '\u4fdd\u5b58\u63a5\u7ba1\u8bbe\u7f6e\u5931\u8d25')
      return
    }
    enabled = response.takeover_enabled === requested
    refreshButtons()
  })
  cookieBtn.addEventListener('click', async () => {
    if (!host) return
    const consentHosts = [...new Set([host, ...resourceHosts].filter(Boolean))]
    const allAuthorized = consentHosts.every(value => authorized.includes(value))
    authorized = allAuthorized
      ? authorized.filter(value => !consentHosts.includes(value))
      : [...new Set([...authorized, ...consentHosts])]
    await browser.storage.local.set({ authorizedCookieHosts: authorized })
    refreshButtons()
  })
  excludeBtn.addEventListener('click', async () => {
    if (!host) return
    excluded = excluded.includes(host) ? excluded.filter(value => value !== host) : [...excluded, host]
    await browser.storage.local.set({ excludedHosts: excluded })
    refreshButtons()
  })

  const [tab] = await browser.tabs.query({ active: true, currentWindow: true })
  const pageUrl = tab?.url || ''
  try { host = new URL(pageUrl).host } catch { host = '' }
  resources = await browser.runtime.sendMessage({ type: 'list', pageUrl, tabId: tab?.id }) || []
  resourceHosts = [...new Set(visibleMediaResources(resources).map(item => {
    try { return new URL(item.url).host } catch { return '' }
  }).filter(Boolean))]
  const online = Boolean((await browser.runtime.sendMessage({ type: 'ping' }))?.ok)
  statusEl.textContent = online ? '\u684c\u9762\u7aef\u5df2\u8fde\u63a5' : '\u684c\u9762\u7aef\u79bb\u7ebf'
  statusEl.classList.toggle('online', online)
  const stored = await browser.storage.local.get(['enabled', 'authorizedCookieHosts', 'excludedHosts'])
  enabled = stored.enabled !== false
  authorized = Array.isArray(stored.authorizedCookieHosts) ? stored.authorizedCookieHosts : []
  excluded = Array.isArray(stored.excludedHosts) ? stored.excludedHosts : []
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
  }, 800)
}

void main()
