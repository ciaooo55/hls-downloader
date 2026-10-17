import { browser } from 'wxt/browser'
import { normalizeHost, visibleMediaResources, type MediaResource } from '../../lib/resources'
import { resourceQuality } from '../../lib/hlsManifest'
import { HANDOFF_SUPPRESSION_STORAGE_KEY, normalizeHandoffSuppressions, type HandoffSuppression } from '../../lib/handoffSuppression'
import { normalizeCookiePermissionHosts } from '../../lib/browserCookies'
import { extensionNeedsUpgrade } from '../../lib/version'
import { engineConnectionLabel, EXTENSION_PRODUCT_LABEL, extensionVersionLabel } from '../../lib/productCopy'
import { withDeadline } from '../../lib/asyncDeadline'
import { waitForDownloadConfirmation } from '../../lib/downloadConfirmation'
import { mediaPushTerminalResult } from '../../lib/mediaPush'
import { formatBytes, formatDuration } from '../../lib/format'
import { mountSubtitlePopup } from '../../lib/subtitlePopup'
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

function updatePopupMaxHeight(main: HTMLElement) {
  // Firefox 的内容 viewport 可能高于原生弹窗；按工作区剩余高度约束，让资源列表滚动。
  const screen = window.screen as Screen & { availTop?: number }
  const firefoxWindow = window as Window & { mozInnerScreenY?: number }
  const viewportTop = firefoxWindow.mozInnerScreenY ?? window.screenY
  const workAreaTop = screen.availTop ?? 0
  const remainingHeight = workAreaTop + screen.availHeight - viewportTop
  // 原生扩展弹窗最高 600 CSS px；超过时浏览器裁切底部，而不会扩大窗口。
  const maxHeight = Math.max(1, Math.min(600, Math.floor(remainingHeight)))
  main.style.setProperty('--popup-max-height', `${maxHeight}px`)
}

const THEME_LABELS: Record<ThemePreference, string> = {
  auto: '主题：自动',
  dark: '主题：深色',
  light: '主题：浅色',
}
const THEME_ORDER: ThemePreference[] = ['auto', 'dark', 'light']

// 按钮状态机使用稳定的状态 token，展示文案单独映射；改文案不能破坏状态判断。
type SendState = 'sending' | 'confirming' | 'queued' | 'retry'
type PushState = 'working' | 'awaiting' | 'sent' | 'retry'
const SEND_LABELS: Record<SendState, string> = { sending: '发送中', confirming: '等待确认', queued: '已加入', retry: '重试' }
const SEND_BUSY: ReadonlySet<SendState> = new Set(['sending', 'confirming', 'queued'])
const TVBOX_PUSH_LABELS: Record<PushState, string> = { working: '推送中', awaiting: '等待选择', sent: '已发送', retry: '重试' }
const CAST_PUSH_LABELS: Record<PushState, string> = { working: '投屏中', awaiting: '等待选择', sent: '已发送', retry: '重试' }
const PUSH_BUSY: ReadonlySet<PushState> = new Set(['working', 'awaiting', 'sent'])
const ICONS: Record<string, Array<['path' | 'circle' | 'rect', Record<string, string>]>> = {
  auto: [['path', { d: 'M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9Z' }]],
  dark: [['path', { d: 'M20.8 15.4A8.6 8.6 0 0 1 8.6 3.2 8.9 8.9 0 1 0 20.8 15.4Z' }]],
  light: [['circle', { cx: '12', cy: '12', r: '4' }], ['path', { d: 'M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4' }]],
  close: [['path', { d: 'm6 6 12 12M18 6 6 18' }]],
  open: [['path', { d: 'M5 12h13m-5-5 5 5-5 5' }]],
  download: [['path', { d: 'M12 4v10m0 0 4-4m-4 4-4-4M5 19h14' }]],
  cast: [['path', { d: 'M5 18a2 2 0 0 1 2 2M5 13a7 7 0 0 1 7 7M5 8a12 12 0 0 1 12 12M19 5H5v3' }]],
  tv: [['rect', { x: '3', y: '5', width: '18', height: '12', rx: '2' }], ['path', { d: 'M8 21h8M12 17v4' }]],
  check: [['path', { d: 'm5 12 4 4L19 6' }]],
  scan: [['path', { d: 'M20 11a8 8 0 1 0-2.34 5.66M20 4v7h-7' }]],
  media: [['rect', { x: '3', y: '5', width: '18', height: '14', rx: '2' }], ['path', { d: 'm10 9 5 3-5 3Z' }]],
  copy: [['rect', { x: '9', y: '9', width: '11', height: '11', rx: '2' }], ['path', { d: 'M5 15V6a2 2 0 0 1 2-2h9' }]],
}
function icon(name: string, label = '') {
  const node = el('span', 'hlsd-icon')
  const namespace = 'http://www.w3.org/2000/svg'
  const svg = document.createElementNS(namespace, 'svg')
  svg.setAttribute('viewBox', '0 0 24 24')
  svg.setAttribute('aria-hidden', 'true')
  for (const [tag, attributes] of ICONS[name] || []) {
    const shape = document.createElementNS(namespace, tag)
    for (const [attribute, value] of Object.entries(attributes)) shape.setAttribute(attribute, value)
    svg.append(shape)
  }
  node.append(svg)
  if (label) node.setAttribute('aria-label', label)
  return node
}

function resourceKindLabel(item: MediaResource) {
  if (item.kind === 'hls') return 'HLS'
  if (item.kind === 'dash') return 'DASH'
  if (item.kind === 'magnet') return '\u78c1\u529b'
  if (item.kind === 'file') return '\u6587\u4ef6'
  const suffix = resourceSuffix(item)
  if (item.mimeType?.toLowerCase().startsWith('audio/') || /^\.(?:aac|flac|m4a|mp3|oga|ogg|opus|wav)$/i.test(suffix)) return '\u97f3\u9891'
  if (item.mimeType?.toLowerCase().startsWith('video/') || /^\.(?:avi|m4v|mkv|mov|mp4|webm)$/i.test(suffix)) return '\u89c6\u9891'
  return '\u5a92\u4f53'
}

function resourceSuffix(item: MediaResource) {
  try {
    const match = new URL(item.url).pathname.match(/(\.[a-z0-9]{1,10})$/i)
    return match?.[1]?.toLowerCase() || ''
  } catch {
    return ''
  }
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
  // Paint the complete shell before the first browser/storage await. An MV3
  // popup may become visible while storage or the service worker is waking;
  // keeping all DOM behind that await left a permanent white page whenever
  // bootstrap was delayed or rejected.
  let themePreference: ThemePreference = 'auto'
  // Every user click bumps this. The async storage restore below captures it and
  // bails out when it is stale, because a restore that resolves after a click
  // still carries the pre-click value and would roll the surface back to it.
  let themeChangeGeneration = 0
  let removeThemeListener = applyTheme(document.documentElement, themePreference)
  const themeBtn = el('button', 'hlsd-button subtle')
  themeBtn.append(icon(themePreference, THEME_LABELS[themePreference]))
  themeBtn.title = THEME_LABELS[themePreference]
  themeBtn.setAttribute('aria-label', themeBtn.title)
  themeBtn.addEventListener('click', async () => {
    themeChangeGeneration += 1
    themePreference = THEME_ORDER[(THEME_ORDER.indexOf(themePreference) + 1) % THEME_ORDER.length]
    removeThemeListener()
    removeThemeListener = applyTheme(document.documentElement, themePreference)
    themeBtn.replaceChildren(icon(themePreference, THEME_LABELS[themePreference]))
    themeBtn.title = THEME_LABELS[themePreference]
    themeBtn.setAttribute('aria-label', themeBtn.title)
    try {
      await browser.storage.local.set({ [THEME_STORAGE_KEY]: themePreference })
    } catch {
      setError('主题已切换，但未能保存；下次打开时可能恢复原设置。')
    }
  })
  const openBtn = el('button', 'hlsd-button desktop-open')
  openBtn.append(icon('open'), el('span', '', '主程序'))
  openBtn.title = '\u6253\u5f00\u684c\u9762\u7aef'
  openBtn.addEventListener('click', async () => {
    openBtn.disabled = true
    openBtn.setAttribute('aria-busy', 'true')
    setError('')
    try {
      const response = await withDeadline(browser.runtime.sendMessage({ type: 'activate' }), 10_000, '打开主程序超时，请重试')
      if (!response?.ok) setError(response?.error || '无法打开主程序')
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '无法打开主程序')
    } finally {
      openBtn.disabled = false
      openBtn.removeAttribute('aria-busy')
    }
  })
  const closeBtn = el('button', 'hlsd-button subtle')
  closeBtn.append(icon('close', '\u5173\u95ed'))
  closeBtn.title = '\u5173\u95ed'
  closeBtn.setAttribute('aria-label', '关闭插件弹窗')
  closeBtn.addEventListener('click', () => window.close())
  actions.append(themeBtn, openBtn, closeBtn)
  header.append(brand, actions)

  const controls = el('div', 'controls')
  const enableBtn = el('button', 'hlsd-button', '\u81ea\u52a8\u63a5\u7ba1')
  const cookieBtn = el('button', 'hlsd-button', '\u672c\u7ad9 Cookie')
  const excludeBtn = el('button', 'hlsd-button', '\u672c\u7ad9\u63d0\u793a')
  // The popup DOM becomes visible before its asynchronous storage/bootstrap
  // work finishes. Do not present controls as clickable until their handlers
  // and local source of truth are ready; a very fast click was otherwise lost.
  enableBtn.disabled = true
  cookieBtn.disabled = true
  excludeBtn.disabled = true
  const settingsPanel = el('section', 'settings-panel')
  const settingsSite = el('p', 'panel-hint', '正在读取当前站点…')
  settingsPanel.append(el('h2', '', '下载与站点设置'), settingsSite)
  const settings = [
    [enableBtn, '自动接管', '全局设置，控制浏览器下载是否交给主程序。'],
    [cookieBtn, '本站 Cookie', '仅在授权后，为本站下载读取资源请求所需的 Cookie。'],
    [excludeBtn, '本站媒体提示', '控制网页上的媒体悬浮入口，不影响插件内识别。'],
  ] as const
  for (const [button, label, description] of settings) {
    const row = el('div', 'setting-item')
    const text = el('div')
    text.append(el('strong', '', label), el('small', '', description))
    row.append(text, button)
    controls.append(row)
  }
  settingsPanel.append(controls)

  const updateNotice = el('div', 'update-notice')
  updateNotice.hidden = true
  const updateText = el('span')
  const updateButton = el('button', '', '查看新版') as HTMLButtonElement
  updateButton.type = 'button'
  updateNotice.append(updateText, updateButton)

  const errorBox = el('div', 'send-error')
  errorBox.hidden = true
  errorBox.setAttribute('role', 'alert')
  const section = el('section', 'resources-panel')
  const sectionHeader = el('div', 'section-header')
  const title = el('div', 'section-title', '当前页面')
  const count = el('b', '', '0')
  const scanBtn = el('button', 'scan-button') as HTMLButtonElement
  scanBtn.type = 'button'
  scanBtn.disabled = true
  scanBtn.append(icon('scan'), el('span', '', '\u91cd\u65b0\u8bc6\u522b'))
  scanBtn.title = '\u91cd\u65b0\u626b\u63cf\u5f53\u524d\u9875\u9762\u7684\u5a92\u4f53\u548c\u4e0b\u8f7d\u8d44\u6e90'
  sectionHeader.append(title, scanBtn)
  const list = el('div', 'list')
  section.append(sectionHeader, list)
  const footer = el('footer')
  const restorePromptsBtn = el('button', 'restore-site-prompts', '\u6062\u590d\u672c\u7ad9\u81ea\u52a8\u63d0\u793a') as HTMLButtonElement
  restorePromptsBtn.type = 'button'
  restorePromptsBtn.hidden = true
  footer.append(
    el('span', '', EXTENSION_PRODUCT_LABEL),
    el('span', '', extensionVersionLabel(browser.runtime.getManifest().version)),
  )
  settingsPanel.append(restorePromptsBtn)
  const subtitlePanel = el('section')
  subtitlePanel.append(el('p', 'panel-hint', '正在读取当前页面…'))
  const views = el('div', 'views')
  const navigation = el('nav', 'view-tabs')
  navigation.setAttribute('role', 'tablist')
  navigation.setAttribute('aria-label', '插件功能')
  const panels = [section, subtitlePanel, settingsPanel]
  const labels = ['资源', '字幕', '设置']
  let subtitleMounted = false
  let pageTabId: number | undefined
  const tabs = labels.map((label, index) => {
    const button = el('button', 'view-tab', label)
    button.type = 'button'
    button.id = `tab-${index}`
    button.setAttribute('role', 'tab')
    button.setAttribute('aria-controls', `panel-${index}`)
    panels[index].id = `panel-${index}`
    panels[index].setAttribute('role', 'tabpanel')
    panels[index].setAttribute('aria-labelledby', button.id)
    return button
  })
  const selectPanel = (index: number, focus = false) => {
    if (index === 1 && !subtitleMounted && pageTabId !== undefined) {
      subtitlePanel.replaceChildren()
      mountSubtitlePopup(subtitlePanel, pageTabId)
      subtitleMounted = true
    }
    panels.forEach((panel, at) => { panel.hidden = at !== index })
    tabs.forEach((tab, at) => {
      tab.setAttribute('aria-selected', String(at === index))
      tab.tabIndex = at === index ? 0 : -1
    })
    if (focus) tabs[index].focus()
  }
  tabs[0].append(count)
  tabs.forEach((tab, index) => {
    tab.addEventListener('click', () => selectPanel(index))
    tab.addEventListener('keydown', event => {
      const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length
        : event.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length
          : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : -1
      if (next < 0) return
      event.preventDefault()
      selectPanel(next, true)
    })
  })
  navigation.append(...tabs)
  views.append(...panels)
  selectPanel(0)
  mainEl.append(header, navigation, updateNotice, errorBox, views, footer)
  updatePopupMaxHeight(mainEl)
  root.append(mainEl)
  window.addEventListener('resize', () => updatePopupMaxHeight(mainEl))
  document.documentElement.dataset.popupReady = 'shell'

  const themeRestoreGeneration = themeChangeGeneration
  void withDeadline(browser.storage.local.get(THEME_STORAGE_KEY), 1_000)
    .then(storedTheme => {
      // A click that happened while this read was in flight already applied and
      // persisted a newer preference; honour it instead of the stale snapshot.
      if (themeRestoreGeneration !== themeChangeGeneration) return
      const restoredTheme = normalizeThemePreference(storedTheme[THEME_STORAGE_KEY])
      if (restoredTheme === themePreference) return
      themePreference = restoredTheme
      removeThemeListener()
      removeThemeListener = applyTheme(document.documentElement, themePreference)
      themeBtn.replaceChildren(icon(themePreference, THEME_LABELS[themePreference]))
      themeBtn.title = THEME_LABELS[themePreference]
      themeBtn.setAttribute('aria-label', themeBtn.title)
    })
    .catch(() => undefined)

  let enabled = true
  let host = ''
  let authorizedCookieHosts: string[] = []
  let excluded: string[] = []
  let suppressions: HandoffSuppression[] = []
  let settingsBusy: HTMLButtonElement | undefined
  const sending: Record<string, SendState> = {}
  const pushing: Record<string, PushState> = {}
  // Chosen rendition per resource id: the list re-renders on every send or
  // status poll, and the pick must survive those rebuilds.
  const chosenVariant: Record<string, string> = {}
  let resources: MediaResource[] = []
  let resourceState: 'loading' | 'ready' | 'scanning' | 'error' = 'loading'
  let canScanPage = false
  let rescanPage = async () => {}

  const statusEl = brandText.querySelector('.status') as HTMLSpanElement
  const setError = (message = '') => {
    errorBox.hidden = !message
    errorBox.textContent = message
  }
  const renderList = () => {
    const focused = document.activeElement instanceof HTMLElement && list.contains(document.activeElement)
      ? { id: document.activeElement.closest<HTMLElement>('article')?.dataset.resourceId, action: document.activeElement.dataset.action }
      : undefined
    const scrollTop = section.scrollTop
    const visible = visibleMediaResources(resources)
    count.textContent = String(visible.length)
    list.replaceChildren()
    if (!visible.length) {
      const empty = el('div', 'empty')
      const emptyIcon = el('div', 'empty-icon')
      emptyIcon.append(icon('media'))
      const heading = resourceState === 'loading'
        ? '\u6b63\u5728\u8bfb\u53d6\u5f53\u524d\u9875\u9762'
        : resourceState === 'scanning'
          ? '\u6b63\u5728\u91cd\u65b0\u8bc6\u522b'
          : resourceState === 'error'
            ? '\u672a\u80fd\u5b8c\u6210\u8bc6\u522b'
            : !canScanPage ? '此页面不支持媒体识别' : '\u5f53\u524d\u9875\u9762\u8fd8\u6ca1\u6709\u53ef\u4e0b\u8f7d\u8d44\u6e90'
      const description = resourceState === 'loading' || resourceState === 'scanning'
        ? '\u6b63\u5728\u68c0\u67e5\u89c6\u9891\u3001\u97f3\u9891\u548c\u5a92\u4f53\u6e05\u5355\u3002'
        : !canScanPage ? '请切换到普通网页，播放媒体后再打开插件。' : '\u5148\u64ad\u653e\u9875\u9762\u4e2d\u7684\u5a92\u4f53\uff0c\u6216\u70b9\u51fb\u4e0b\u65b9\u91cd\u65b0\u8bc6\u522b\u3002'
      empty.append(emptyIcon, el('strong', '', heading), el('span', '', description))
      if (canScanPage && resourceState !== 'loading' && resourceState !== 'scanning') {
        const retry = el('button', 'empty-retry') as HTMLButtonElement
        retry.type = 'button'
        retry.append(icon('scan'), el('span', '', '\u91cd\u65b0\u8bc6\u522b'))
        retry.addEventListener('click', () => void rescanPage())
        empty.append(retry)
      }
      list.append(empty)
      return
    }
    for (const item of visible) {
      let selected = item
      const remembered = chosenVariant[item.id]
      if (remembered) {
        const variant = item.variants?.find(value => value.url === remembered)
        if (variant) selected = { ...item, ...variant, url: variant.url, variants: undefined }
      }
      let itemHost = selected.url
      try { itemHost = new URL(selected.url).host } catch {}
      const likelySize = selected.size || selected.estimatedSize || 0
      const size = selected.size && selected.size > 0 ? formatBytes(selected.size) : likelySize > 0 ? `约 ${formatBytes(likelySize)}` : '大小未知'
      const quality = selected.quality || resourceQuality(selected.url, selected.height)
      const resolution = selected.width && selected.height ? (selected.width + '\u00d7' + selected.height) : ''
      const bandwidth = selected.bandwidth ? ((selected.bandwidth / 1_000_000).toFixed(1) + ' Mbps') : ''
      const duration = selected.duration ? formatDuration(selected.duration) : ''
      const streamMode = item.isLive === true ? '直播' : ''
      const meta = [resourceKindLabel(item), resourceSuffix(item), streamMode, quality, resolution, bandwidth, duration, size].filter(Boolean).join(' \u00b7 ')
      const article = el('article')
      article.dataset.resourceId = item.id
      article.tabIndex = -1
      const body = el('div', 'resource-body')
      const name = el('strong', '', item.title || item.filename || item.url.split('/').pop() || item.url)
      const cardUrl = item.url
      name.title = item.filename || item.title || cardUrl
      const line = el('span', '', meta)
      line.title = meta
      const mime = el('small', '', itemHost)
      mime.title = [item.mimeType, itemHost].filter(Boolean).join(' \u00b7 ')
      const copyBtn = el('button', 'copy-link') as HTMLButtonElement
      copyBtn.type = 'button'
      copyBtn.dataset.action = 'copy'
      copyBtn.append(icon('copy'), el('span', 'copy-label', '\u590d\u5236\u94fe\u63a5'))
      copyBtn.title = itemHost ? `\u590d\u5236\u5b8c\u6574\u94fe\u63a5\uff08${itemHost}\uff09` : '\u590d\u5236\u5b8c\u6574\u94fe\u63a5'
      copyBtn.addEventListener('click', () => void copyLink(selected.url, copyBtn))
      body.append(name, line)
      if (item.variants?.length) {
        const variantLabel = (variant?: { quality?: string; height?: number; bandwidth?: number }) =>
          variant
            ? [variant.quality || (variant.height ? `${variant.height}p` : '\u7ebf\u8def'), variant.bandwidth ? `${(variant.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''].filter(Boolean).join(' \u00b7 ')
            : '\u81ea\u52a8\uff08\u6700\u9ad8\uff09'
        // 原生选择框提供方向键、Escape 和越过滚动容器的菜单，不再维护自绘遮罩。
        const trigger = el('select', 'quality-trigger')
        trigger.dataset.action = 'quality'
        trigger.setAttribute('aria-label', '选择视频清晰度')
        trigger.append(new Option(variantLabel(), item.url))
        for (const variant of item.variants) trigger.append(new Option(variantLabel(variant), variant.url))
        trigger.value = selected.url
        trigger.addEventListener('change', () => {
          if (trigger.value === item.url) delete chosenVariant[item.id]
          else chosenVariant[item.id] = trigger.value
          renderList()
        })
        body.append(trigger)
      }
      const metaRow = el('div', 'resource-meta')
      metaRow.append(mime, copyBtn)
      body.append(metaRow)
      const sendState = sending[item.id]
      const label = sendState ? SEND_LABELS[sendState] : '下载'
      const button = el('button', 'hlsd-button primary')
      button.dataset.state = sendState || ''
      const sendPending = sendState === 'sending' || sendState === 'confirming'
      button.dataset.action = 'download'
      button.append(icon(sendPending ? 'scan' : sendState === 'queued' ? 'check' : 'download'), el('span', '', label))
      const locked = sendState ? SEND_BUSY.has(sendState) : false
      button.disabled = locked
      button.classList.toggle('busy', sendPending)
      button.setAttribute('aria-busy', String(sendPending))
      button.title = sendState === 'confirming' ? '请在主程序中确认本次下载' : '发送到主程序，确认后加入下载队列'
      button.addEventListener('click', () => void send(selected))
      const pushKey = `${item.id}:tvbox`
      const pushState = pushing[pushKey]
      const pushLabel = pushState ? TVBOX_PUSH_LABELS[pushState] : 'TVBox'
      const pushButton = el('button', 'hlsd-button push-button')
      pushButton.dataset.state = pushState || ''
      const pushPending = pushState === 'working' || pushState === 'awaiting'
      pushButton.dataset.action = 'tvbox'
      pushButton.append(icon(pushPending ? 'scan' : pushState === 'sent' ? 'check' : 'tv'), el('span', '', pushLabel))
      pushButton.disabled = pushState ? PUSH_BUSY.has(pushState) : false
      pushButton.classList.toggle('busy', pushPending)
      pushButton.setAttribute('aria-busy', String(pushPending))
      pushButton.title = '直接推送当前媒体链接到 TVBox'
      pushButton.addEventListener('click', () => void pushToTv(selected))
      const castKey = `${item.id}:cast`
      const castState = pushing[castKey]
      const castLabel = castState ? CAST_PUSH_LABELS[castState] : '投屏'
      const castButton = el('button', 'hlsd-button cast-button')
      castButton.dataset.state = castState || ''
      const castPending = castState === 'working' || castState === 'awaiting'
      castButton.dataset.action = 'cast'
      castButton.append(icon(castPending ? 'scan' : castState === 'sent' ? 'check' : 'cast'), el('span', '', castLabel))
      castButton.disabled = castState ? PUSH_BUSY.has(castState) : false
      castButton.classList.toggle('busy', castPending)
      castButton.setAttribute('aria-busy', String(castPending))
      castButton.title = '直接投屏当前媒体链接到 DLNA 或 Chromecast'
      castButton.addEventListener('click', () => void castToDevice(selected))
      const actionCol = el('div', 'article-actions')
      actionCol.append(button, castButton, pushButton)
      article.append(body, actionCol)
      list.append(article)
    }
    section.scrollTop = scrollTop
    if (focused?.id) {
      const card = [...list.querySelectorAll('article')].find(node => node.dataset.resourceId === focused.id)
      const control = [...(card?.querySelectorAll<HTMLButtonElement | HTMLSelectElement>('[data-action]') || [])]
        .find(node => node.dataset.action === focused.action && !node.disabled)
      const target = control || card
      target?.focus({ preventScroll: true })
    }
  }

  const refreshButtons = () => {
    // 这些按钮显示“当前状态”，不是动作。加前缀 + aria-pressed，避免被读成
    // “点击后会发生什么”的开关。
    enableBtn.textContent = enabled ? '已开启' : '已关闭'
    enableBtn.setAttribute('aria-label', `自动接管：${enableBtn.textContent}`)
    enableBtn.title = enabled ? '全局下载接管已开启；点击关闭' : '全局下载接管已关闭；点击开启'
    enableBtn.setAttribute('aria-pressed', String(enabled))
    enableBtn.classList.toggle('active', enabled)
    const cookieAuthorized = Boolean(host && authorizedCookieHosts.includes(host))
    cookieBtn.textContent = cookieAuthorized ? '已授权' : '未授权'
    cookieBtn.setAttribute('aria-label', `本站 Cookie：${cookieBtn.textContent}`)
    cookieBtn.title = cookieAuthorized
      ? '\u5df2\u6388\u6743\uff1a\u53d1\u9001\u672c\u7ad9\u5a92\u4f53\u65f6\u4f1a\u53ea\u8bfb\u53d6\u6d4f\u89c8\u5668\u5bf9\u8be5\u8d44\u6e90\u5730\u5740\u53d1\u9001\u7684 Cookie\uff1b\u70b9\u51fb\u64a4\u9500\u6388\u6743'
      : '\u672a\u6388\u6743\uff1a\u9ed8\u8ba4\u4e0d\u8bfb\u53d6 Cookie\uff1b\u70b9\u51fb\u6388\u6743\u5f53\u524d\u7ad9\u70b9'
    cookieBtn.setAttribute('aria-pressed', String(cookieAuthorized))
    cookieBtn.classList.toggle('active', cookieAuthorized)
    const siteExcluded = excluded.includes(host)
    excludeBtn.textContent = siteExcluded ? '已隐藏' : '已显示'
    excludeBtn.setAttribute('aria-label', `本站提示：${excludeBtn.textContent}`)
    excludeBtn.title = siteExcluded ? '\u5f53\u524d\u5df2\u5728\u672c\u7ad9\u9690\u85cf\u81ea\u52a8\u63d0\u793a\uff1b\u70b9\u51fb\u6062\u590d' : '\u5f53\u524d\u4f1a\u5728\u672c\u7ad9\u663e\u793a\u81ea\u52a8\u63d0\u793a\uff1b\u70b9\u51fb\u9690\u85cf'
    excludeBtn.setAttribute('aria-pressed', String(siteExcluded))
    excludeBtn.classList.toggle('active', siteExcluded)
    const suppressedKinds = suppressions.filter(rule => rule.host === host).map(rule => rule.kind)
    restorePromptsBtn.hidden = suppressedKinds.length === 0
    restorePromptsBtn.title = suppressedKinds.length
      ? `\u6062\u590d\u672c\u7ad9 ${suppressedKinds.join('\u3001')} \u8d44\u6e90\u7684\u81ea\u52a8\u63d0\u793a`
      : ''
    for (const button of [enableBtn, cookieBtn, excludeBtn, restorePromptsBtn]) {
      button.disabled = Boolean(settingsBusy) || (button !== enableBtn && !host)
      button.setAttribute('aria-busy', String(settingsBusy === button))
    }
  }

  const waitForPushResult = async (requestId: string) => {
    const deadline = Date.now() + 130_000
    while (Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 1_000))
      const response = await browser.runtime.sendMessage({ type: 'media-push-status', requestId }).catch(() => null)
      const terminal = mediaPushTerminalResult(response)
      if (terminal) return terminal
    }
    return { status: 'pending', message: '桌面端尚未完成设备选择' }
  }

  const pushToTv = async (item: MediaResource) => {
    const key = `${item.id}:tvbox`
    const current = pushing[key]
    if (current && PUSH_BUSY.has(current)) return
    setError('')
    pushing[key] = 'working'
    renderList()
    try {
      const response = await browser.runtime.sendMessage({ type: 'push-to-tv', resource: item })
      if (!response?.ok) throw new Error(response?.error || '电视推送失败')
      pushing[key] = 'awaiting'
      renderList()
      const status = await waitForPushResult(String(response.id || ''))
      if (status.status !== 'done') throw new Error(status.message || '电视推送未完成')
      pushing[key] = 'sent'
      setError('')
      setTimeout(() => { delete pushing[key]; renderList() }, 2_000)
    } catch (reason) {
      pushing[key] = 'retry'
      setError(reason instanceof Error ? reason.message : '推送到电视失败')
    } finally {
      renderList()
    }
  }

  const castToDevice = async (item: MediaResource) => {
    const key = `${item.id}:cast`
    const current = pushing[key]
    if (current && PUSH_BUSY.has(current)) return
    setError('')
    pushing[key] = 'working'
    renderList()
    try {
      const response = await browser.runtime.sendMessage({ type: 'cast-to-device', resource: item })
      if (!response?.ok) throw new Error(response?.error || '投屏请求失败')
      pushing[key] = 'awaiting'
      renderList()
      const status = await waitForPushResult(String(response.id || ''))
      if (status.status !== 'done') throw new Error(status.message || '投屏未完成')
      pushing[key] = 'sent'
      setError('')
      setTimeout(() => { delete pushing[key]; renderList() }, 2_000)
    } catch (reason) {
      pushing[key] = 'retry'
      setError(reason instanceof Error ? reason.message : '投屏到电视失败')
    } finally {
      renderList()
    }
  }

  const copyResetTimers = new WeakMap<HTMLButtonElement, ReturnType<typeof setTimeout>>()
  const copyLink = async (url: string, node: HTMLButtonElement) => {
    const label = node.querySelector<HTMLSpanElement>('.copy-label')
    const done = (text: string) => {
      clearTimeout(copyResetTimers.get(node))
      if (label) label.textContent = text
      node.classList.add('copied')
      copyResetTimers.set(node, setTimeout(() => {
        if (label) label.textContent = '复制链接'
        node.classList.remove('copied')
        copyResetTimers.delete(node)
      }, 1_800))
    }
    try {
      // navigator.clipboard needs a focused document; the popup already is one,
      // but keep the textarea fallback for browsers that deny the async API.
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(url)
      } else {
        const scratch = document.createElement('textarea')
        scratch.value = url
        scratch.setAttribute('readonly', '')
        scratch.style.position = 'fixed'
        scratch.style.opacity = '0'
        document.body.append(scratch)
        scratch.select()
        try {
          if (!document.execCommand('copy')) throw new Error('Copy was denied')
        } finally {
          scratch.remove()
        }
      }
      done('\u5df2\u590d\u5236')
    } catch {
      setError('\u590d\u5236\u94fe\u63a5\u5931\u8d25\uff0c\u8bf7\u624b\u52a8\u9009\u62e9')
    }
  }

  const confirmDownload = async (item: MediaResource, handoffId: string) => {
    sending[item.id] = 'confirming'
    renderList()
    if (!await waitForDownloadConfirmation(message => browser.runtime.sendMessage(message), handoffId)) {
      delete sending[item.id]
      renderList()
      return
    }
    sending[item.id] = 'queued'
    renderList()
    setTimeout(() => { delete sending[item.id]; renderList() }, 2_500)
  }

  const send = async (item: MediaResource) => {
    if (sending[item.id] && SEND_BUSY.has(sending[item.id])) return
    setError('')
    sending[item.id] = 'sending'
    renderList()
    try {
      const response = await withDeadline(
        browser.runtime.sendMessage({ type: 'download-now', resource: item }),
        10_000,
        '下载器响应超时，请重试',
      )
      if (!response?.ok) throw new Error(response?.error || '桌面端未接受下载请求')
      await confirmDownload(item, String(response.handoff?.id || ''))
    } catch (reason) {
      sending[item.id] = 'retry'
      setError(reason instanceof Error ? reason.message : '\u53d1\u9001\u5230\u684c\u9762\u7aef\u5931\u8d25')
      renderList()
    }
  }

  enableBtn.addEventListener('click', async () => {
    if (settingsBusy) return
    const requested = !enabled
    settingsBusy = enableBtn
    refreshButtons()
    setError('')
    try {
      const response = await withDeadline(browser.runtime.sendMessage({ type: 'set-takeover-settings', enabled: requested }), 10_000, '保存接管设置超时，请重试')
      if (!response?.ok) {
        setError(response?.error || '\u4fdd\u5b58\u63a5\u7ba1\u8bbe\u7f6e\u5931\u8d25')
        return
      }
      if (typeof response.takeover_enabled !== 'boolean') {
        setError('\u684c\u9762\u7aef\u672a\u8fd4\u56de\u6709\u6548\u7684\u63a5\u7ba1\u8bbe\u7f6e')
        return
      }
      enabled = response.takeover_enabled
      if (enabled !== requested) setError('\u63a5\u7ba1\u8bbe\u7f6e\u5df2\u88ab\u684c\u9762\u7aef\u7684\u66f4\u65b0\u503c\u8986\u76d6')
      refreshButtons()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '\u4fdd\u5b58\u63a5\u7ba1\u8bbe\u7f6e\u5931\u8d25')
    } finally {
      settingsBusy = undefined
      refreshButtons()
    }
  })
  cookieBtn.addEventListener('click', async () => {
    if (!host || settingsBusy) return
    const next = authorizedCookieHosts.includes(host)
      ? authorizedCookieHosts.filter(value => value !== host)
      : normalizeCookiePermissionHosts([...authorizedCookieHosts, host])
    settingsBusy = cookieBtn
    refreshButtons()
    setError('')
    try {
      await browser.storage.local.set({ authorizedCookieHosts: next, useBrowserCookies: false })
      authorizedCookieHosts = next
      refreshButtons()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Cookie 设置保存失败')
    } finally {
      settingsBusy = undefined
      refreshButtons()
    }
  })
  excludeBtn.addEventListener('click', async () => {
    if (!host || settingsBusy) return
    const next = excluded.includes(host) ? excluded.filter(value => value !== host) : [...excluded, host]
    settingsBusy = excludeBtn
    refreshButtons()
    setError('')
    try {
      await browser.storage.local.set({ excludedHosts: next })
      excluded = next
      refreshButtons()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '保存排除站点失败')
    } finally {
      settingsBusy = undefined
      refreshButtons()
    }
  })
  restorePromptsBtn.addEventListener('click', async () => {
    if (!host || settingsBusy) return
    const next = suppressions.filter(rule => rule.host !== host)
    settingsBusy = restorePromptsBtn
    refreshButtons()
    setError('')
    try {
      await browser.storage.local.set({ [HANDOFF_SUPPRESSION_STORAGE_KEY]: next })
      suppressions = next
      refreshButtons()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '恢复本站提示失败')
    } finally {
      settingsBusy = undefined
      refreshButtons()
    }
  })

  const windowTabs = await withDeadline(browser.tabs.query({ currentWindow: true }), 1_000).catch(() => [])
  // A normal toolbar popup leaves the web tab active. Developer tools and
  // automated browser checks can open popup.html as an extension tab instead;
  // in that case fall back to the most recently listed web tab rather than
  // showing a misleading zero-resource state for the extension page itself.
  const tab = windowTabs.find(candidate => candidate.active && /^https?:\/\//i.test(candidate.url || ''))
    || [...windowTabs].reverse().find(candidate => /^https?:\/\//i.test(candidate.url || ''))
    || windowTabs.find(candidate => candidate.active)
  const pageUrl = tab?.url || ''
  pageTabId = /^https?:\/\//i.test(pageUrl) ? tab?.id : undefined
  if (tabs[1].getAttribute('aria-selected') === 'true') selectPanel(1)
  if (pageTabId === undefined) subtitlePanel.replaceChildren(el('p', 'panel-hint', '请在包含播放器的网页中使用字幕翻译。'))
  host = normalizeHost(pageUrl)
  settingsSite.textContent = host ? `当前站点 · ${host}` : '当前页面不是普通网页，站点设置不可用。'
  title.textContent = host || '当前页面不支持媒体识别'
  title.title = pageUrl
  const downloadErrorKey = `download-error:${tab?.id}`
  void browser.storage.session.get(downloadErrorKey).then(stored => {
    if (stored[downloadErrorKey]) setError(String(stored[downloadErrorKey]))
  }).catch(() => undefined)
  browser.storage.onChanged.addListener((changes, area) => {
    if (area === 'session' && changes[downloadErrorKey]) setError(String(changes[downloadErrorKey].newValue || ''))
  })
  canScanPage = Number.isInteger(tab?.id) && /^https?:\/\//i.test(pageUrl)
  rescanPage = async () => {
    if (!canScanPage || tab?.id == null || resourceState === 'scanning') return
    resourceState = 'scanning'
    scanBtn.disabled = true
    setError('')
    renderList()
    try {
      await withDeadline(
        browser.tabs.sendMessage(tab.id, { type: 'rescan-media' }),
        1_500,
        '\u9875\u9762\u8bc6\u522b\u54cd\u5e94\u8d85\u65f6',
      )
      const listed = await withDeadline(
        browser.runtime.sendMessage({ type: 'list', pageUrl, tabId: tab.id }),
        1_500,
        '\u8bfb\u53d6\u8bc6\u522b\u7ed3\u679c\u8d85\u65f6',
      )
      resources = Array.isArray(listed) ? listed : []
      resourceState = 'ready'
    } catch (reason) {
      resourceState = 'error'
      setError(reason instanceof Error ? reason.message : '\u91cd\u65b0\u8bc6\u522b\u5931\u8d25')
    } finally {
      scanBtn.disabled = !canScanPage
      renderList()
    }
  }
  scanBtn.disabled = !canScanPage
  scanBtn.addEventListener('click', () => void rescanPage())
  renderList()
  // Load local settings before any network/native request. The popup is often
  // opened exactly while a page starts a download; controls must be usable even
  // if the desktop ping or resource query is temporarily slow.
  const stored = await withDeadline(browser.storage.local.get([
    'enabled', 'excludedHosts', 'authorizedCookieHosts', HANDOFF_SUPPRESSION_STORAGE_KEY,
  ]), 1_000).catch(() => ({} as Record<string, unknown>))
  enabled = stored.enabled !== false
  authorizedCookieHosts = normalizeCookiePermissionHosts(stored.authorizedCookieHosts)
  excluded = Array.isArray(stored.excludedHosts)
    ? stored.excludedHosts.map(value => normalizeHost(String(value || ''))).filter(Boolean)
    : []
  suppressions = normalizeHandoffSuppressions(stored[HANDOFF_SUPPRESSION_STORAGE_KEY])
  enableBtn.disabled = false
  refreshButtons()
  const connectionRequest = browser.runtime.sendMessage({ type: 'ping' })
  const [listed, connection] = await Promise.all([
    withDeadline(browser.runtime.sendMessage({ type: 'list', pageUrl, tabId: tab?.id }), 1_500).catch(() => []),
    withDeadline(connectionRequest, 1_800).catch(() => ({ reconnecting: true })),
  ])
  resources = Array.isArray(listed) ? listed : []
  resourceState = 'ready'
  function showConnection(response: any) {
    const online = Boolean(response?.ok)
    statusEl.textContent = engineConnectionLabel(online, Boolean(response?.reconnecting))
    statusEl.classList.toggle('online', online)
    const recommendedExtensionVersion = String(response?.recommended_extension_version || '')
    const extensionReleaseUrl = String(response?.extension_release_url || '')
    if (extensionNeedsUpgrade(browser.runtime.getManifest().version, recommendedExtensionVersion)) {
      updateNotice.hidden = false
      updateText.textContent = `插件可升级到版本 ${recommendedExtensionVersion}。商店安装版由浏览器自动更新；开发者模式安装请下载新版后重新加载。`
      updateButton.hidden = !extensionReleaseUrl
      updateButton.onclick = () => {
        if (extensionReleaseUrl) void browser.tabs.create({ url: extensionReleaseUrl })
      }
    }
  }
  showConnection(connection)
  // 冷启动超过界面等待期限时，仍接收原请求的真实结果，避免一直显示“重连中”。
  if (connection.reconnecting) void connectionRequest.then(showConnection).catch(() => showConnection({ ok: false }))
  refreshButtons()
  renderList()
  document.documentElement.dataset.popupReady = 'ready'
  void browser.runtime.sendMessage({ type: 'pending-downloads', tabId: tab?.id }).then(pending => {
    if (!Array.isArray(pending)) return
    for (const handoff of pending) {
      const item = resources.find(resource => resource.id === handoff.resourceId)
      if (!item || sending[item.id]) continue
      void confirmDownload(item, String(handoff.id)).catch(reason => {
        sending[item.id] = 'retry'
        setError(reason instanceof Error ? reason.message : '无法读取下载确认状态')
        renderList()
      })
    }
  }).catch(() => undefined)
}

function renderStartupError(reason: unknown) {
  const shell = el('main', 'popup-boot')
  const header = el('div', 'popup-boot__header')
  const logo = el('img') as HTMLImageElement
  logo.src = '/icon-32.png'
  logo.alt = ''
  header.append(logo, el('div', 'popup-boot__title', 'HLS Downloader'))
  const body = el('div', 'popup-boot__body')
  const message = el('div')
  message.append(
    el('strong', '', '插件界面加载失败'),
    el('span', '', reason instanceof Error && reason.message
      ? reason.message
      : '请重新打开插件；问题仍存在时重新加载扩展。'),
  )
  body.append(message)
  shell.append(header, body)
  root.replaceChildren(shell)
  document.documentElement.dataset.popupReady = 'error'
}

void main().catch(renderStartupError)
