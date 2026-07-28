import { browser } from 'wxt/browser'
import { visibleMediaResources, type MediaResource } from '../../lib/resources'
import { resourceQuality } from '../../lib/hlsManifest'
import { handoffStatusLabel, handoffTerminalStatus } from '../../lib/takeover'
import { HANDOFF_SUPPRESSION_STORAGE_KEY, normalizeHandoffSuppressions, type HandoffSuppression } from '../../lib/handoffSuppression'
import {
  THEME_BASE_CSS,
  THEME_STORAGE_KEY,
  THEME_TOKENS_CSS,
  applyTheme,
  normalizeThemePreference,
  type ThemePreference,
} from '../../lib/theme'
import './style.css'

const root = document.getElementById('root')!
const tokenStyle = document.createElement('style')
tokenStyle.textContent = THEME_TOKENS_CSS + THEME_BASE_CSS
document.head.append(tokenStyle)

const THEME_LABELS: Record<ThemePreference, string> = {
  auto: '主题：自动',
  dark: '主题：深色',
  light: '主题：浅色',
}
const THEME_ORDER: ThemePreference[] = ['auto', 'dark', 'light']
const THEME_GLYPHS: Record<ThemePreference, string> = { auto: '◐', dark: '●', light: '○' }
const PENDING_HANDOFF_STORAGE_KEY = 'popup-pending-handoffs-v1'

interface PendingHandoff {
  handoffId: string
  startedAt: number
}

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
  let themePreference = normalizeThemePreference(
    (await browser.storage.local.get(THEME_STORAGE_KEY))[THEME_STORAGE_KEY],
  )
  let removeThemeListener = applyTheme(document.documentElement, themePreference)
  const themeBtn = el('button', 'hlsd-button subtle', THEME_GLYPHS[themePreference])
  themeBtn.title = THEME_LABELS[themePreference]
  themeBtn.addEventListener('click', async () => {
    themePreference = THEME_ORDER[(THEME_ORDER.indexOf(themePreference) + 1) % THEME_ORDER.length]
    removeThemeListener()
    removeThemeListener = applyTheme(document.documentElement, themePreference)
    themeBtn.textContent = THEME_GLYPHS[themePreference]
    themeBtn.title = THEME_LABELS[themePreference]
    await browser.storage.local.set({ [THEME_STORAGE_KEY]: themePreference })
  })
  const openBtn = el('button', 'hlsd-button primary', '\u6253\u5f00')
  openBtn.title = '\u6253\u5f00\u684c\u9762\u7aef'
  openBtn.addEventListener('click', () => void browser.runtime.sendMessage({ type: 'activate' }))
  const closeBtn = el('button', 'hlsd-button subtle', '\u00d7')
  closeBtn.title = '\u5173\u95ed'
  closeBtn.addEventListener('click', () => window.close())
  actions.append(themeBtn, openBtn, closeBtn)
  header.append(brand, actions)

  const controls = el('div', 'controls')
  const enableBtn = el('button', 'hlsd-button', '\u81ea\u52a8\u63a5\u7ba1')
  const cookieBtn = el('button', 'hlsd-button', 'Cookie')
  const excludeBtn = el('button', 'hlsd-button', '\u6392\u9664\u672c\u7ad9')
  controls.append(enableBtn, cookieBtn, excludeBtn)

  const errorBox = el('div', 'send-error')
  errorBox.hidden = true
  const section = el('section')
  const title = el('div', 'section-title', '\u5f53\u524d\u9875\u9762\u8d44\u6e90 ')
  const count = el('b', '', '0')
  title.append(count)
  const list = el('div', 'list')
  section.append(title, list)
  const footer = el('footer')
  const restorePromptsBtn = el('button', 'restore-site-prompts', '\u6062\u590d\u672c\u7ad9\u81ea\u52a8\u63d0\u793a') as HTMLButtonElement
  restorePromptsBtn.type = 'button'
  restorePromptsBtn.hidden = true
  footer.append(
    el('span', '', 'Alt \u7ed5\u8fc7 \u00b7 Ctrl \u5f3a\u5236\u63a5\u7ba1'),
    el('span', '', `v${browser.runtime.getManifest().version}`),
    restorePromptsBtn,
  )
  mainEl.append(header, controls, errorBox, section, footer)
  root.append(mainEl)

  let enabled = true
  let host = ''
  let useBrowserCookies = true
  let excluded: string[] = []
  let suppressions: HandoffSuppression[] = []
  const sending: Record<string, string> = {}
  const pending: Record<string, PendingHandoff> = {}
  const pendingFailures: Record<string, number> = {}
  const pushing: Record<string, string> = {}
  // Chosen rendition per resource id: the list re-renders on every send or
  // status poll, and the pick must survive those rebuilds.
  const chosenVariant: Record<string, string> = {}
  let resources: MediaResource[] = []

  const statusEl = brandText.querySelector('.status') as HTMLSpanElement
  const setError = (message = '') => {
    errorBox.hidden = !message
    errorBox.textContent = message
  }
  const persistPending = () => browser.storage.session.set({ [PENDING_HANDOFF_STORAGE_KEY]: pending }).catch(() => undefined)
  const clearPending = (resourceId: string) => {
    delete pending[resourceId]
    delete pendingFailures[resourceId]
    void persistPending()
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
      const likelySize = item.size || item.estimatedSize || 0
      const size = item.size && item.size > 0 ? formatSize(item.size) : likelySize > 0 ? `\u7ea6 ${formatSize(likelySize)}` : '\u5927\u5c0f\u672a\u77e5'
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
      let selected = item
      const remembered = chosenVariant[item.id]
      if (remembered) {
        const variant = item.variants?.find(value => value.url === remembered)
        if (variant) selected = { ...item, ...variant, url: variant.url, variants: undefined }
      }
      body.append(name, line)
      if (item.variants?.length) {
        const variantLabel = (variant?: { quality?: string; height?: number; bandwidth?: number }) =>
          variant
            ? [variant.quality || (variant.height ? `${variant.height}p` : '\u7ebf\u8def'), variant.bandwidth ? `${(variant.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''].filter(Boolean).join(' \u00b7 ')
            : '\u81ea\u52a8\uff08\u6700\u9ad8\uff09'
        const trigger = el('button', 'quality-trigger') as HTMLButtonElement
        trigger.type = 'button'
        trigger.setAttribute('aria-label', '\u9009\u62e9\u89c6\u9891\u6e05\u6670\u5ea6')
        const currentLabel = el('em', '', variantLabel(
          item.variants?.find(value => value.url === chosenVariant[item.id]),
        ))
        trigger.append(currentLabel, el('span', '', '\u25be'))
        trigger.addEventListener('click', event => {
          event.stopPropagation()
          const host = trigger.closest('article') as HTMLElement | null
          if (!host) return
          const open = host.querySelector('.quality-menu')
          document.querySelectorAll('.quality-menu, .quality-menu-backdrop').forEach(node => node.remove())
          if (open) return
          const backdrop = el('div', 'quality-menu-backdrop')
          const menu = el('div', 'quality-menu')
          backdrop.addEventListener('mousedown', () => { backdrop.remove(); menu.remove() })
          const choices: Array<{ url: string; label: string; variant?: NonNullable<MediaResource['variants']>[number] }> = [
            { url: item.url, label: variantLabel() },
            ...item.variants!.map(variant => ({ url: variant.url, label: variantLabel(variant), variant })),
          ]
          for (const choice of choices) {
            const option = el('button') as HTMLButtonElement
            option.type = 'button'
            option.append(el('i', '', selected.url === choice.url ? '\u2713' : ''), el('span', '', choice.label))
            option.addEventListener('click', () => {
              selected = choice.variant ? { ...item, ...choice.variant, url: choice.variant.url, variants: undefined } : item
              if (choice.variant) chosenVariant[item.id] = choice.variant.url
              else delete chosenVariant[item.id]
              currentLabel.textContent = choice.label
              backdrop.remove(); menu.remove()
            })
            menu.append(option)
          }
          host.append(backdrop, menu)
          menu.style.top = `${trigger.offsetTop + trigger.offsetHeight + 4}px`
          menu.style.left = `${trigger.offsetLeft}px`
        })
        body.append(trigger)
      }
      body.append(mime)
      const label = sending[item.id] || '\u4e0b\u8f7d'
      const button = el('button', 'hlsd-button primary', label)
      const locked = ['\u53d1\u9001\u4e2d', '\u7b49\u5f85\u786e\u8ba4', '\u786e\u8ba4\u4e2d', '\u5df2\u52a0\u5165'].includes(sending[item.id] || '')
      button.disabled = locked
      if (sending[item.id]) button.classList.add('busy')
      button.title = '\u53d1\u9001\u5230\u4e0b\u8f7d\u5668'
      button.addEventListener('click', () => void send(selected))
      const pushLabel = pushing[item.id] || '推电视'
      const pushButton = el('button', 'hlsd-button push-button', pushLabel)
      pushButton.disabled = pushing[item.id] === '推送中'
      if (pushing[item.id]) pushButton.classList.add('busy')
      pushButton.title = '推送到电视播放'
      pushButton.addEventListener('click', () => void pushToTv(selected))
      const actionCol = el('div', 'article-actions')
      actionCol.append(button, pushButton)
      article.append(body, actionCol)
      list.append(article)
    }
  }

  const refreshButtons = () => {
    enableBtn.textContent = enabled ? '\u81ea\u52a8\u63a5\u7ba1\u5f00' : '\u81ea\u52a8\u63a5\u7ba1\u5173'
    enableBtn.classList.toggle('active', enabled)
    cookieBtn.textContent = useBrowserCookies ? '\u7f51\u9875 Cookie \u5f00' : '\u7f51\u9875 Cookie \u5173'
    cookieBtn.title = useBrowserCookies ? '\u53d1\u9001\u5a92\u4f53\u5230\u684c\u9762\u7aef\u65f6\uff0c\u4f7f\u7528\u6d4f\u89c8\u5668\u5bf9\u8be5\u5a92\u4f53\u5730\u5740\u4f1a\u53d1\u9001\u7684 Cookie' : '\u4e0d\u4f20\u9012\u6d4f\u89c8\u5668 Cookie\uff0c\u53ef\u80fd\u964d\u4f4e\u767b\u5f55\u7ad9\u70b9\u4e0b\u8f7d\u6210\u529f\u7387'
    cookieBtn.classList.toggle('active', useBrowserCookies)
    cookieBtn.disabled = false
    const siteExcluded = excluded.includes(host)
    excludeBtn.textContent = siteExcluded ? '\u672c\u7ad9\u5df2\u6392\u9664' : '\u6392\u9664\u672c\u7ad9'
    excludeBtn.classList.toggle('active', siteExcluded)
    excludeBtn.disabled = !host
    const suppressedKinds = suppressions.filter(rule => rule.host === host).map(rule => rule.kind)
    restorePromptsBtn.hidden = suppressedKinds.length === 0
    restorePromptsBtn.title = suppressedKinds.length
      ? `\u6062\u590d\u672c\u7ad9 ${suppressedKinds.join('\u3001')} \u8d44\u6e90\u7684\u81ea\u52a8\u63d0\u793a`
      : ''
  }

  const pushToTv = async (item: MediaResource) => {
    setError('')
    pushing[item.id] = '等待选择'
    renderList()
    try {
      const response = await browser.runtime.sendMessage({ type: 'push-to-tv', resource: item })
      if (!response?.ok) throw new Error(response?.error || '电视推送失败')
      pushing[item.id] = '已发送'
    } catch (reason) {
      pushing[item.id] = '重试'
      setError(reason instanceof Error ? reason.message : '推送到电视失败')
    } finally {
      renderList()
      setTimeout(() => { delete pushing[item.id]; renderList() }, 1500)
    }
  }

  const send = async (item: MediaResource) => {
    setError('')
    sending[item.id] = '\u53d1\u9001\u4e2d'
    renderList()
    try {
      const response = await browser.runtime.sendMessage({ type: 'offer', resource: item })
      if (!response?.ok || !response?.handoff?.id) throw new Error(response?.error || '\u684c\u9762\u7aef\u6ca1\u6709\u521b\u5efa\u4e0b\u8f7d\u7a97\u53e3')
      sending[item.id] = '\u7b49\u5f85\u786e\u8ba4'
      pending[item.id] = { handoffId: response.handoff.id, startedAt: Date.now() }
      void persistPending()
      renderList()
    } catch (reason) {
      sending[item.id] = '\u91cd\u8bd5'
      clearPending(item.id)
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
    useBrowserCookies = !useBrowserCookies
    await browser.storage.local.set({ useBrowserCookies })
    refreshButtons()
  })
  excludeBtn.addEventListener('click', async () => {
    if (!host) return
    excluded = excluded.includes(host) ? excluded.filter(value => value !== host) : [...excluded, host]
    await browser.storage.local.set({ excludedHosts: excluded })
    refreshButtons()
  })
  restorePromptsBtn.addEventListener('click', async () => {
    if (!host) return
    suppressions = suppressions.filter(rule => rule.host !== host)
    await browser.storage.local.set({ [HANDOFF_SUPPRESSION_STORAGE_KEY]: suppressions })
    refreshButtons()
  })

  const [tab] = await browser.tabs.query({ active: true, currentWindow: true })
  const pageUrl = tab?.url || ''
  try { host = new URL(pageUrl).host } catch { host = '' }
  resources = await browser.runtime.sendMessage({ type: 'list', pageUrl, tabId: tab?.id }) || []
  const online = Boolean((await browser.runtime.sendMessage({ type: 'ping' }))?.ok)
  statusEl.textContent = online ? '\u684c\u9762\u7aef\u5df2\u8fde\u63a5' : '\u684c\u9762\u7aef\u79bb\u7ebf'
  statusEl.classList.toggle('online', online)
  const stored = await browser.storage.local.get([
    'enabled', 'excludedHosts', 'useBrowserCookies', HANDOFF_SUPPRESSION_STORAGE_KEY,
  ])
  enabled = stored.enabled !== false
  useBrowserCookies = stored.useBrowserCookies !== false
  excluded = Array.isArray(stored.excludedHosts) ? stored.excludedHosts : []
  suppressions = normalizeHandoffSuppressions(stored[HANDOFF_SUPPRESSION_STORAGE_KEY])
  const session = await browser.storage.session.get(PENDING_HANDOFF_STORAGE_KEY)
  const restored = session[PENDING_HANDOFF_STORAGE_KEY]
  if (restored && typeof restored === 'object') {
    for (const [resourceId, value] of Object.entries(restored as Record<string, Partial<PendingHandoff>>)) {
      const handoffId = String(value?.handoffId || '')
      const startedAt = Number(value?.startedAt || 0)
      if (!handoffId || !Number.isFinite(startedAt) || startedAt <= 0) continue
      pending[resourceId] = { handoffId, startedAt }
      sending[resourceId] = '\u7b49\u5f85\u786e\u8ba4'
    }
  }
  refreshButtons()
  renderList()

  window.setInterval(() => {
    const entries = Object.entries(pending)
    if (!entries.length) return
    void Promise.all(entries.map(async ([resourceId, pendingItem]) => {
      if (Date.now() - pendingItem.startedAt > 130_000) {
        sending[resourceId] = '\u5df2\u8fc7\u671f'
        clearPending(resourceId)
        setError('桌面端确认超时，请重试或打开下载器查看状态')
        renderList()
        return
      }
      try {
        const response = await browser.runtime.sendMessage({ type: 'handoff-status', handoffId: pendingItem.handoffId })
        const handoff = response?.handoff || response
        const status = String(handoff?.status || '')
        if (!handoffTerminalStatus(status)) return
        sending[resourceId] = handoffStatusLabel(status)
        clearPending(resourceId)
        if (status === 'accepted') setError('')
        if (status === 'connection_lost') setError('桌面端连接中断，请重试')
        renderList()
      } catch {
        pendingFailures[resourceId] = (pendingFailures[resourceId] || 0) + 1
        if (pendingFailures[resourceId] < 3) return
        sending[resourceId] = '\u91cd\u8bd5'
        clearPending(resourceId)
        setError('无法读取桌面端确认状态，请重试')
        renderList()
      }
    }))
  }, 800)
}

void main()
