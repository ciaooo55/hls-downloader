import { browser } from 'wxt/browser'
import {
  downloadSelectionKey,
  isDefaultPopupResource,
  isPlayableMediaResource,
  normalizeHost,
  resourceFingerprint,
  selectMediaVariant,
  visibleMediaResources,
  type MediaResource,
  type MediaVariant,
} from '../../lib/resources'
import { patchKeyedChildren, setAttrIfChanged, setTextIfChanged, syncSelectOptions } from '../../lib/domPatch'
import { resourceQuality } from '../../lib/hlsManifest'
import { HANDOFF_SUPPRESSION_STORAGE_KEY, hostForPage, normalizeHandoffSuppressions, type HandoffSuppression } from '../../lib/handoffSuppression'
import { normalizeCookiePermissionHosts } from '../../lib/browserCookies'
import { extensionNeedsUpgrade } from '../../lib/version'
import {
  classifyEngineConnectionState,
  engineStatusCopy,
  EXTENSION_PRODUCT_LABEL,
  extensionVersionLabel,
  type EngineConnectionState,
} from '../../lib/productCopy'
import { withDeadline } from '../../lib/asyncDeadline'
import { waitForDownloadConfirmation } from '../../lib/downloadConfirmation'
import { mediaPushTerminalResult } from '../../lib/mediaPush'
import { formatBitrate, formatBytes, formatDuration } from '../../lib/format'
import { countPopupCategories, filterPopupResources, type ResourceFilterCategory } from '../../lib/popupFilter'
import { mountSubtitlePopup } from '../../lib/subtitlePopup'
import {
  canForceResend,
  SEND_LABELS,
  SEND_LOCKED,
  SEND_PENDING,
  SEND_TITLES,
  sendClickAction,
  sendStateFromHandoff,
  type SendState,
} from '../../lib/downloadSendState'
import { DESKTOP_RELEASES_URL, launchDesktop, launchView, type LaunchAction, type LaunchState } from '../../lib/desktopLaunch'
import {
  THEME_BASE_CSS,
  THEME_STORAGE_KEY,
  THEME_TOKENS_CSS,
  applyTheme,
  normalizeThemePreference,
  readCachedResolvedTheme,
  writeCachedResolvedTheme,
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
  const maxHeight = Number.isFinite(remainingHeight) && remainingHeight >= 240
    ? Math.min(600, Math.floor(remainingHeight))
    : 600
  main.style.setProperty('--popup-max-height', `${maxHeight}px`)
}

const THEME_LABELS: Record<ThemePreference, string> = {
  auto: '主题：自动',
  dark: '主题：深色',
  light: '主题：浅色',
}
const THEME_ORDER: ThemePreference[] = ['auto', 'dark', 'light']

type PushState = 'working' | 'awaiting' | 'sent' | 'retry'
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
  node.dataset.iconName = name
  node.dataset.iconLabel = label
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

function patchIcon(container: HTMLElement, name: string, label = '') {
  const current = container.querySelector<HTMLElement>(':scope > .hlsd-icon')
  if (current && current.dataset.iconName === name && (current.dataset.iconLabel || '') === label) {
    return current
  }
  const next = icon(name, label)
  if (current) current.replaceWith(next)
  else container.prepend(next)
  return next
}

function resourceKindLabel(item: MediaResource) {
  if (item.kind === 'hls') return 'HLS'
  if (item.kind === 'dash') return 'DASH'
  if (item.kind === 'magnet') return '\u78c1\u529b'
  if (item.kind === 'file') return '\u6587\u4ef6'
  const suffix = resourceSuffix(item)
  if (item.mimeType?.toLowerCase().startsWith('audio/') || /^\.(?:aac|flac|m4a|mp3|oga|ogg|opus|wav)$/i.test(suffix)) return '\u97f3\u9891'
  if (item.mimeType?.toLowerCase().startsWith('video/') || /^\.(?:avi|m4s|m4v|mkv|mov|mp4|webm)$/i.test(suffix)) return '\u89c6\u9891'
  return '\u5a92\u4f53'
}

function resourceSuffix(item: MediaResource) {
  if (item.audioUrl) return '.mp4'
  try {
    const match = new URL(item.url).pathname.match(/(\.[a-z0-9]{1,10})$/i)
    const raw = match?.[1]?.toLowerCase() || ''
    if (raw === '.m4s' && item.kind === 'media') return '.mp4'
    return raw
  } catch {
    return ''
  }
}

function compactQualityLabel(variant: MediaVariant): string {
  const raw = String(variant.quality || '').replace(/^最高\s*/, '').trim()
  const height = Number(variant.height || 0)
  if (/^2160p$/i.test(raw) || (!raw && height >= 2160)) return '4K'
  if (/^1440p$/i.test(raw) || (!raw && height >= 1440)) return '2K'
  if (raw) return raw.replace(/^(\d{3,4})p(\d*)$/i, (_, h, fps) => `${h}P${fps}`)
  if (height > 0) return `${height}P`
  return variant.bandwidth ? formatBitrate(variant.bandwidth) : '线路'
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
  brandText.append(el('h1', '', 'HLS Downloader'), el('span', 'status reconnecting', '\u8fde\u63a5\u4e2d\u2026'))
  brand.append(logo, brandText)
  const actions = el('div', 'header-actions')

  let themePreference: ThemePreference = 'auto'
  let themeChangeGeneration = 0
  // Preserve the synchronous <head> cached resolved theme until storage.local resolves
  // so manual light/dark overrides never flash during boot.
  const bootResolvedTheme = readCachedResolvedTheme()
  let removeThemeListener = applyTheme(
    document.documentElement,
    bootResolvedTheme ?? themePreference,
    undefined,
    writeCachedResolvedTheme,
  )
  const themeBtn = el('button', 'hlsd-button subtle')
  themeBtn.type = 'button'
  themeBtn.append(icon(themePreference, THEME_LABELS[themePreference]))
  themeBtn.title = THEME_LABELS[themePreference]
  themeBtn.setAttribute('aria-label', themeBtn.title)
  themeBtn.addEventListener('click', async () => {
    themeChangeGeneration += 1
    themePreference = THEME_ORDER[(THEME_ORDER.indexOf(themePreference) + 1) % THEME_ORDER.length]
    removeThemeListener()
    removeThemeListener = applyTheme(
      document.documentElement,
      themePreference,
      undefined,
      writeCachedResolvedTheme,
    )
    patchIcon(themeBtn, themePreference, THEME_LABELS[themePreference])
    themeBtn.title = THEME_LABELS[themePreference]
    themeBtn.setAttribute('aria-label', themeBtn.title)
    try {
      await browser.storage.local.set({ [THEME_STORAGE_KEY]: themePreference })
    } catch {
      setError('主题已切换，但未能保存；下次打开时可能恢复原设置。')
    }
  })

  const openBtn = el('button', 'hlsd-button desktop-open')
  openBtn.type = 'button'
  const openBtnLabel = el('span', '', '主程序')
  openBtn.append(icon('open'), openBtnLabel)
  openBtn.title = '\u6253\u5f00\u684c\u9762\u7aef'

  // 内联连接状态助手卡（托盘退出可唤醒 / 未安装桌面端 / 引擎离线重试）
  const connectionHelper = el('div', 'connection-helper')
  connectionHelper.hidden = true
  connectionHelper.setAttribute('role', 'status')
  connectionHelper.setAttribute('aria-live', 'polite')
  const helperDot = el('span', 'connection-helper-dot')
  const helperBody = el('div', 'connection-helper-body')
  const helperTitleEl = el('strong')
  const helperDetailEl = el('span')
  const helperActions = el('div', 'connection-helper-actions')
  helperBody.append(helperTitleEl, helperDetailEl, helperActions)
  connectionHelper.append(helperDot, helperBody)

  // 启动反馈条
  const launchPanel = el('div', 'launch-status')
  launchPanel.hidden = true
  launchPanel.setAttribute('role', 'status')
  launchPanel.setAttribute('aria-live', 'polite')
  let launchRun = 0
  let launchHideTimer: ReturnType<typeof setTimeout> | undefined
  const LAUNCH_ACTION_LABELS: Record<LaunchAction, string> = {
    retry: '重试', keep_waiting: '继续等待', install: '下载桌面端', repair: '修复安装',
  }
  let renderConnectionStatus = () => {}
  const renderLaunch = (state: LaunchState, run: number) => {
    if (run !== launchRun) return
    const view = launchView(state)
    const busy = view.tone === 'progress'
    openBtn.disabled = busy
    openBtn.classList.toggle('busy', busy)
    openBtn.dataset.state = busy ? 'busy' : view.tone === 'success' ? 'ready' : view.tone === 'error' ? 'failed' : ''
    openBtn.setAttribute('aria-busy', String(busy))
    patchIcon(openBtn, busy ? 'scan' : view.tone === 'success' ? 'check' : 'open')
    setTextIfChanged(openBtnLabel, busy ? '打开中' : view.tone === 'success' ? '已打开' : '主程序')
    launchPanel.dataset.tone = view.tone
    const badge = el('span', 'launch-icon')
    badge.append(icon(busy ? 'scan' : view.tone === 'success' ? 'check' : view.tone === 'error' ? 'close' : 'open'))
    const text = el('div', 'launch-text')
    text.append(el('strong', '', view.title))
    if (view.detail) text.append(el('span', '', view.detail))
    const actionsRow = el('div', 'launch-actions')
    for (const action of view.actions) {
      const button = el('button', `hlsd-button${action === view.actions[0] ? ' primary' : ''}`, LAUNCH_ACTION_LABELS[action]) as HTMLButtonElement
      button.type = 'button'
      button.dataset.launchAction = action
      button.addEventListener('click', () => {
        if (action === 'install' || action === 'repair') {
          void browser.tabs.create({ url: DESKTOP_RELEASES_URL }).catch(() => undefined)
          return
        }
        void startLaunch(action === 'keep_waiting' ? state.pending : undefined)
      })
      actionsRow.append(button)
    }
    if (actionsRow.childElementCount) text.append(actionsRow)
    launchPanel.replaceChildren(badge, text)
    launchPanel.hidden = false
    renderConnectionStatus()
    if (launchHideTimer) clearTimeout(launchHideTimer)
    if (view.tone === 'success') {
      launchHideTimer = setTimeout(() => {
        if (run !== launchRun) return
        launchPanel.hidden = true
        openBtn.dataset.state = ''
        patchIcon(openBtn, 'open')
        setTextIfChanged(openBtnLabel, '主程序')
        renderConnectionStatus()
      }, 2_500)
    }
    const failedAction = launchPanel.querySelector<HTMLButtonElement>('[data-launch-action]')
    if (view.tone === 'error' && document.activeElement === document.body) failedAction?.focus({ preventScroll: true })
  }
  let recheckConnection = async () => {}
  const startLaunch = async (resumeFrom?: LaunchState['pending']) => {
    const run = ++launchRun
    setError('')
    await launchDesktop({
      send: message => withDeadline(browser.runtime.sendMessage(message), 15_000, '主程序响应超时'),
      onState: state => {
        renderLaunch(state, run)
        if (state.stage === 'ready') void recheckConnection()
      },
      resumeFrom,
    }).catch(reason => renderLaunch({ stage: 'failed', failure: 'unknown', detail: reason instanceof Error ? reason.message : '' }, run))
  }
  openBtn.addEventListener('click', () => {
    if (openBtn.getAttribute('aria-busy') === 'true') return
    void startLaunch()
  })
  const closeBtn = el('button', 'hlsd-button subtle')
  closeBtn.type = 'button'
  closeBtn.append(icon('close', '\u5173\u95ed'))
  closeBtn.title = '\u5173\u95ed'
  closeBtn.setAttribute('aria-label', '关闭插件弹窗')
  closeBtn.addEventListener('click', () => window.close())
  actions.append(themeBtn, openBtn, closeBtn)
  header.append(brand, actions)

  const controls = el('div', 'controls')
  const enableBtn = el('button', 'hlsd-button', '已开启')
  enableBtn.type = 'button'
  enableBtn.setAttribute('aria-label', '全局自动接管：已开启')
  const quickDownloadBtn = el('button', 'hlsd-button', '静默快速下载')
  quickDownloadBtn.type = 'button'
  const speedLimitBtn = el('button', 'hlsd-button', '下载限速')
  speedLimitBtn.type = 'button'
  const cookieBtn = el('button', 'hlsd-button', '\u672c\u7ad9 Cookie')
  cookieBtn.type = 'button'
  const excludeBtn = el('button', 'hlsd-button', '\u672c\u7ad9\u63d0\u793a')
  excludeBtn.type = 'button'
  enableBtn.disabled = true
  quickDownloadBtn.disabled = true
  speedLimitBtn.disabled = true
  cookieBtn.disabled = true
  excludeBtn.disabled = true
  const settingsPanel = el('section', 'settings-panel')
  const settingsSite = el('p', 'panel-hint', '正在读取当前站点…')
  settingsPanel.append(el('h2', '', '下载与站点设置'), settingsSite)
  let speedLimitDesc!: HTMLElement
  const settings = [
    [enableBtn, '自动接管', '全局设置，控制浏览器下载是否交给主程序。'],
    [quickDownloadBtn, '静默快速下载（不弹确认框）', '全局设置，接管后直接加入下载队列，跳过确认弹窗。'],
    [speedLimitBtn, '下载限速', '全局限速开关，与主程序限速状态保持同步。'],
    [cookieBtn, '本站 Cookie', '仅在授权后，为本站下载读取资源请求所需的 Cookie。'],
    [excludeBtn, '本站提示与接管', '在此网站隐藏悬浮下载按钮，并暂停自动接管下载。'],
  ] as const
  for (const [button, label, description] of settings) {
    const row = el('div', 'setting-item')
    const text = el('div')
    const descEl = el('small', '', description)
    if (button === speedLimitBtn) speedLimitDesc = descEl
    text.append(el('strong', '', label), descEl)
    row.append(text, button)
    controls.append(row)
  }
  settingsPanel.append(controls)

  // 紧凑常驻快捷开关条（panel-0 激活时单行直达核心全局开关，无折叠遮挡）
  const quickDrawer = el('div', 'quick-settings-bar')
  quickDrawer.setAttribute('role', 'group')
  quickDrawer.setAttribute('aria-label', '快捷控制')
  const quickRow = el('div', 'quick-settings-row')
  const quickEnablePill = el('button', 'quick-setting-pill', '自动接管: 开') as HTMLButtonElement
  quickEnablePill.type = 'button'
  quickEnablePill.setAttribute('aria-label', '自动接管：开')
  quickEnablePill.disabled = true
  const quickSilentPill = el('button', 'quick-setting-pill', '静默下载: 关') as HTMLButtonElement
  quickSilentPill.type = 'button'
  quickSilentPill.setAttribute('aria-label', '静默下载：关')
  quickSilentPill.disabled = true
  const quickSpeedPill = el('button', 'quick-setting-pill', '限速: 关') as HTMLButtonElement
  quickSpeedPill.type = 'button'
  quickSpeedPill.setAttribute('aria-label', '下载限速：关')
  quickSpeedPill.disabled = true
  quickEnablePill.addEventListener('click', () => enableBtn.click())
  quickSilentPill.addEventListener('click', () => quickDownloadBtn.click())
  quickSpeedPill.addEventListener('click', () => speedLimitBtn.click())
  quickRow.append(quickEnablePill, quickSilentPill, quickSpeedPill)
  quickDrawer.append(quickRow)

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
  scanBtn.append(icon('scan'), el('span', '', '嗅探全部资源'))
  scanBtn.title = '\u91cd\u65b0\u626b\u63cf\u5f53\u524d\u9875\u9762\u7684\u5a92\u4f53\u548c\u4e0b\u8f7d\u8d44\u6e90'
  sectionHeader.append(title, scanBtn)
  const resourceToolbar = el('div', 'resource-toolbar')
  resourceToolbar.hidden = true
  const filterRow = el('div', 'resource-filter-row')
  const filterSearch = el('input', 'resource-search') as HTMLInputElement
  filterSearch.type = 'search'
  filterSearch.placeholder = '筛选标题、清晰度或域名…'
  filterSearch.setAttribute('aria-label', '筛选当前页面资源')
  const batchBtn = el('button', 'hlsd-button batch-download-btn') as HTMLButtonElement
  batchBtn.type = 'button'
  batchBtn.title = '将当前筛选出的可下载资源批量交给桌面端队列'
  const batchBtnLabel = el('span', '', '批量下载 (0)')
  batchBtn.append(icon('download'), batchBtnLabel)
  filterRow.append(filterSearch, batchBtn)
  const filterChips = el('div', 'resource-chips')
  filterChips.setAttribute('role', 'group')
  filterChips.setAttribute('aria-label', '按类型筛选资源')
  resourceToolbar.append(filterRow, filterChips)
  const cookieAuthBanner = el('div', 'cookie-auth-banner')
  cookieAuthBanner.hidden = true
  const cookieAuthText = el('span', '', '检测到本站登录态 · 高画质或受限媒体可能需授权 Cookie')
  const cookieAuthQuickBtn = el('button', 'hlsd-button', '授权本站') as HTMLButtonElement
  cookieAuthQuickBtn.type = 'button'
  cookieAuthQuickBtn.addEventListener('click', () => cookieBtn.click())
  cookieAuthBanner.append(cookieAuthText, cookieAuthQuickBtn)
  const COOKIE_DOWNGRADED_HOSTS_KEY = 'cookieDowngradedHosts'
  let siteHasCookies = false
  let cookieDowngradedHosts: string[] = []
  const list = el('div', 'list')
  const browserTransfers = el('div', 'browser-transfers')
  browserTransfers.hidden = true
  browserTransfers.setAttribute('aria-live', 'polite')
  section.append(sectionHeader, resourceToolbar, cookieAuthBanner, browserTransfers, list)
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
  let activePanelIndex = 0
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
  let syncEnableAriaLabels = () => {}
  const selectPanel = (index: number, focus = false) => {
    activePanelIndex = index
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
    quickDrawer.hidden = index !== 0
    syncEnableAriaLabels()
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
  mainEl.append(header, connectionHelper, launchPanel, navigation, updateNotice, errorBox, views, quickDrawer, footer)
  updatePopupMaxHeight(mainEl)
  root.append(mainEl)
  window.addEventListener('resize', () => updatePopupMaxHeight(mainEl))
  document.documentElement.dataset.popupReady = 'shell'

  const themeRestoreGeneration = themeChangeGeneration
  void withDeadline(browser.storage.local.get(THEME_STORAGE_KEY), 1_000)
    .then(storedTheme => {
      if (themeRestoreGeneration !== themeChangeGeneration) return
      const restoredTheme = normalizeThemePreference(storedTheme[THEME_STORAGE_KEY])
      themePreference = restoredTheme
      removeThemeListener()
      removeThemeListener = applyTheme(
        document.documentElement,
        themePreference,
        undefined,
        writeCachedResolvedTheme,
      )
      patchIcon(themeBtn, themePreference, THEME_LABELS[themePreference])
      themeBtn.title = THEME_LABELS[themePreference]
      themeBtn.setAttribute('aria-label', themeBtn.title)
    })
    .catch(() => undefined)

  let enabled = true
  let quickDownloadEnabled = false
  let speedLimitEnabled = false
  let speedLimitKib = 0
  let tvboxConfigured: boolean | undefined
  let connectionState: EngineConnectionState = 'core_unreachable'
  let connectionReconnecting = false
  let connectionChecked = false
  let host = ''
  let suppressionHost = ''
  let authorizedCookieHosts: string[] = []
  let excluded: string[] = []
  let suppressions: HandoffSuppression[] = []
  let settingsBusy: HTMLButtonElement | undefined
  const sending: Record<string, SendState> = {}
  const sentUrls: Record<string, string> = {}
  const pushing: Record<string, PushState> = {}
  let batchRunning = false
  const actionKey = (item: MediaResource, action = 'download') => JSON.stringify([item.id, downloadSelectionKey(item), action])
  const manualCopy = new Set<string>()
  const copyStates = new Map<string, 'copying' | 'copied'>()
  const copyResetTimers = new Map<string, ReturnType<typeof setTimeout>>()
  const renderCopyButton = (button: HTMLButtonElement, url: string) => {
    const state = copyStates.get(url)
    setAttrIfChanged(button, 'aria-disabled', String(state === 'copying'))
    setAttrIfChanged(button, 'aria-busy', String(state === 'copying'))
    button.classList.toggle('copied', state === 'copied')
    patchIcon(button, state === 'copied' ? 'check' : 'copy')
    let labelEl = button.querySelector<HTMLSpanElement>('.copy-label')
    if (!labelEl) {
      labelEl = el('span', 'copy-label', '')
      button.append(labelEl)
    }
    setTextIfChanged(labelEl, state === 'copying' ? '复制中…' : state === 'copied' ? '已复制' : '复制链接')
  }
  const setCopyState = (url: string, state?: 'copying' | 'copied') => {
    if (state) copyStates.set(url, state)
    else copyStates.delete(url)
    for (const button of list.querySelectorAll<HTMLButtonElement>('[data-action=copy]')) {
      if (button.dataset.copyUrl === url) renderCopyButton(button, url)
    }
  }
  const chosenVariant: Record<string, string> = {}
  let filterCategory: ResourceFilterCategory = 'all'
  let filterQuery = ''
  let resources: MediaResource[] = []
  let showCandidates = false
  let resourceState: 'loading' | 'ready' | 'scanning' | 'error' = 'loading'
  let canScanPage = false
  let rescanPage = async () => {}

  const resolveSelectedVariant = (item: MediaResource): MediaResource => {
    const remembered = chosenVariant[item.id]
    if (!remembered) return item
    const variant = item.variants?.find(value => downloadSelectionKey(selectMediaVariant(item, value)) === remembered)
    return variant ? selectMediaVariant(item, variant) : item
  }

  filterSearch.addEventListener('input', () => {
    filterQuery = filterSearch.value
    renderList()
  })

  const statusEl = brandText.querySelector('.status') as HTMLSpanElement
  renderConnectionStatus = () => {
    if (!connectionChecked) return
    if (connectionReconnecting && connectionState !== 'online') {
      statusEl.textContent = '正在检测下载引擎…'
      statusEl.className = 'status reconnecting'
      statusEl.dataset.connection = 'reconnecting'
      connectionHelper.hidden = true
      return
    }
    const copy = engineStatusCopy(connectionState)
    const connectionOnline = connectionState === 'online'
    const speedSuffix = connectionOnline && speedLimitEnabled
      ? (speedLimitKib > 0 ? ` · 限速 ${speedLimitKib} KiB/s` : ' · 限速中')
      : ''
    statusEl.textContent = `${copy.label}${speedSuffix}`
    statusEl.classList.toggle('online', connectionOnline)
    statusEl.classList.toggle('standby', connectionState === 'user_shutdown')
    statusEl.classList.remove('reconnecting')
    statusEl.dataset.connection = connectionState
    const detailParts = [copy.label]
    if (connectionOnline && speedLimitEnabled) {
      detailParts.push(speedLimitKib > 0 ? `全局限速已开启（${speedLimitKib} KiB/s）` : '全局限速已开启')
    }
    if (quickDownloadEnabled) detailParts.push('静默快速下载已开启')
    statusEl.title = detailParts.join(' · ')

    if (connectionOnline || !launchPanel.hidden) {
      connectionHelper.hidden = true
      return
    }
    connectionHelper.dataset.tone = copy.tone
    setTextIfChanged(helperTitleEl, copy.helperTitle)
    setTextIfChanged(helperDetailEl, copy.helperDetail)
    const actionDefs: Array<{ key: string; label: string; primary?: boolean; run: () => void }> =
      connectionState === 'user_shutdown'
        ? [{ key: 'wake', label: '打开工作台', run: () => void startLaunch() }]
        : connectionState === 'host_missing'
          ? [
              { key: 'install', label: '下载桌面端', primary: true, run: () => void browser.tabs.create({ url: DESKTOP_RELEASES_URL }).catch(() => undefined) },
              { key: 'recheck', label: '重新检测', run: () => void recheckConnection() },
            ]
          : [
              { key: 'recheck', label: '重试连接', primary: true, run: () => void recheckConnection() },
              { key: 'open', label: '打开桌面端', run: () => void startLaunch() },
            ]
    patchKeyedChildren(
      helperActions,
      actionDefs,
      entry => entry.key,
      entry => {
        const btn = el('button', `hlsd-button${entry.primary ? ' primary' : ''}`, entry.label) as HTMLButtonElement
        btn.type = 'button'
        btn.dataset.helperAction = entry.key
        return btn
      },
      (btn, entry) => {
        btn.className = `hlsd-button${entry.primary ? ' primary' : ''}`
        setTextIfChanged(btn, entry.label)
        btn.onclick = entry.run
      },
    )
    connectionHelper.hidden = false
  }

  const setError = (message = '') => {
    errorBox.hidden = !message
    errorBox.textContent = message
  }

  interface PopupCardState {
    item: MediaResource
    selected: MediaResource
    name: HTMLElement
    line: HTMLSpanElement
    facts: HTMLSpanElement
    lastMeta: string
    drmBadge: HTMLElement
    qualityChips: HTMLDivElement
    qualityTrigger: HTMLSelectElement
    mime: HTMLElement
    manualCopyHelp: HTMLElement
    manualCopyInput: HTMLInputElement
    downloadBtn: HTMLButtonElement
    downloadLabel: HTMLSpanElement
    resendBtn: HTMLButtonElement
    copyBtn: HTMLButtonElement
    castBtn: HTMLButtonElement
    castLabel: HTMLSpanElement
    pushBtn: HTMLButtonElement
    pushLabel: HTMLSpanElement
  }
  const popupCardStates = new WeakMap<HTMLElement, PopupCardState>()

  const variantLabel = (variant?: { quality?: string; height?: number; bandwidth?: number }) =>
    variant
      ? [variant.quality || (variant.height ? `${variant.height}p` : '\u7ebf\u8def'), variant.bandwidth ? formatBitrate(variant.bandwidth) : ''].filter(Boolean).join(' \u00b7 ')
      : '\u81ea\u52a8\uff08\u6700\u9ad8\uff09'

  const createResourceCard = (item: MediaResource): HTMLElement => {
    const article = el('article')
    article.tabIndex = -1
    const body = el('div', 'resource-body')
    const name = el('strong')
    const line = el('span', 'resource-facts')
    const facts = el('span', 'resource-facts-row')
    facts.setAttribute('aria-hidden', 'true')
    line.append(facts)
    const qualityChips = el('div', 'quality-chips')
    qualityChips.setAttribute('role', 'group')
    qualityChips.setAttribute('aria-label', '一键选择清晰度并下载')
    qualityChips.hidden = true
    const qualityTrigger = el('select', 'quality-trigger') as HTMLSelectElement
    qualityTrigger.dataset.action = 'quality'
    qualityTrigger.setAttribute('aria-label', '选择视频清晰度')
    qualityTrigger.hidden = true
    const metaRow = el('div', 'resource-meta')
    const mime = el('small')
    metaRow.append(mime)
    const manualCopyHelp = el('small', 'manual-copy-help', '自动复制失败，请按 Ctrl+C 复制下方完整链接')
    manualCopyHelp.setAttribute('role', 'status')
    manualCopyHelp.hidden = true
    const manualCopyInput = el('input', 'manual-copy') as HTMLInputElement
    manualCopyInput.type = 'text'
    manualCopyInput.readOnly = true
    manualCopyInput.dataset.action = 'manual-copy'
    manualCopyInput.setAttribute('aria-label', '完整媒体链接，按 Ctrl+C 复制')
    manualCopyInput.hidden = true
    manualCopyInput.addEventListener('focus', () => manualCopyInput.select())
    const drmBadge = el('div', 'drm-badge', 'DRM 加密保护 · 不支持直接下载')
    drmBadge.hidden = true
    body.append(name, line, drmBadge, qualityChips, qualityTrigger, metaRow, manualCopyHelp, manualCopyInput)

    const downloadBtn = el('button', 'hlsd-button primary') as HTMLButtonElement
    downloadBtn.type = 'button'
    downloadBtn.dataset.action = 'download'
    const downloadLabel = el('span', '', '下载')
    downloadBtn.append(icon('download'), downloadLabel)

    const resendBtn = el('button', 'hlsd-button resend-button') as HTMLButtonElement
    resendBtn.type = 'button'
    resendBtn.dataset.action = 'resend'
    resendBtn.hidden = true
    resendBtn.title = '重新发送当前捕获地址（可用于刷新过期链接或再次下载）'
    resendBtn.append(icon('scan'), el('span', '', '重发'))

    const copyBtn = el('button', 'copy-link') as HTMLButtonElement
    copyBtn.type = 'button'
    copyBtn.dataset.action = 'copy'
    copyBtn.append(icon('copy'), el('span', 'copy-label', '复制链接'))

    const castBtn = el('button', 'hlsd-button cast-button') as HTMLButtonElement
    castBtn.type = 'button'
    castBtn.dataset.action = 'cast'
    const castLabel = el('span', '', '投屏')
    castBtn.append(icon('cast'), castLabel)
    castBtn.title = '直接投屏当前媒体链接到 DLNA 或 Chromecast'

    const pushBtn = el('button', 'hlsd-button push-button') as HTMLButtonElement
    pushBtn.type = 'button'
    pushBtn.dataset.action = 'tvbox'
    const pushLabel = el('span', '', 'TVBox')
    pushBtn.append(icon('tv'), pushLabel)
    pushBtn.title = '直接推送当前媒体链接到 TVBox'

    const actionCol = el('div', 'article-actions')
    const more = el('details', 'resource-more')
    const summary = el('summary', '', '更多')
    summary.dataset.action = 'more'
    summary.setAttribute('aria-label', '更多操作')
    const extra = el('div', 'resource-extra')
    extra.append(copyBtn, castBtn, pushBtn)
    more.append(summary, extra)
    actionCol.append(downloadBtn, more)
    article.append(body, actionCol)

    const state: PopupCardState = {
      item,
      selected: resolveSelectedVariant(item),
      name,
      line,
      facts,
      lastMeta: '',
      drmBadge,
      qualityChips,
      qualityTrigger,
      mime,
      manualCopyHelp,
      manualCopyInput,
      downloadBtn,
      downloadLabel,
      resendBtn,
      copyBtn,
      castBtn,
      castLabel,
      pushBtn,
      pushLabel,
    }
    popupCardStates.set(article, state)

    qualityTrigger.addEventListener('change', () => {
      if (qualityTrigger.value === downloadSelectionKey(state.item)) delete chosenVariant[state.item.id]
      else chosenVariant[state.item.id] = qualityTrigger.value
      renderList()
    })
    copyBtn.addEventListener('click', () => void copyLink(state.selected.url))
    downloadBtn.addEventListener('click', () => {
      const key = actionKey(state.selected)
      const sendState = sending[key]
      const urlChanged = Boolean(sentUrls[key] && sentUrls[key] !== state.selected.url)
      const action = sendClickAction(sendState, { urlChanged })
      if (action === 'open_desktop') void startLaunch()
      else if (action !== 'none') void send(state.selected, { forceResend: urlChanged })
    })
    resendBtn.addEventListener('click', () => {
      void send(state.selected, { forceResend: true })
    })
    pushBtn.addEventListener('click', () => void pushToTv(state.selected))
    castBtn.addEventListener('click', () => void castToDevice(state.selected))

    return article
  }

  const updateResourceCard = (article: HTMLElement, item: MediaResource) => {
    const state = popupCardStates.get(article)
    if (!state) return
    const selected = resolveSelectedVariant(item)
    state.item = item
    state.selected = selected

    if (article.dataset.resourceId !== item.id) article.dataset.resourceId = item.id
    const selectionKey = downloadSelectionKey(selected)
    if (article.dataset.selectionKey !== selectionKey) article.dataset.selectionKey = selectionKey

    let itemHost = selected.url
    try { itemHost = new URL(selected.url).host } catch {}
    const likelySize = selected.size || selected.estimatedSize || 0
    const size = selected.size && selected.size > 0 ? formatBytes(selected.size) : likelySize > 0 ? `约 ${formatBytes(likelySize)}` : '大小未知'
    const quality = selected.quality || resourceQuality(selected.url, selected.height)
    const resolution = selected.width && selected.height ? (selected.width + '\u00d7' + selected.height) : ''
    const bandwidth = selected.bandwidth ? formatBitrate(selected.bandwidth) : ''
    const duration = selected.duration ? formatDuration(selected.duration) : ''
    const streamMode = selected.isLive === true ? '直播' : ''
    const dualTrackTag = selected.audioUrl ? '音画合并' : ''
    const metaParts = [resourceKindLabel(selected), resourceSuffix(selected), streamMode, dualTrackTag, quality, resolution, bandwidth, duration, size].filter(Boolean)
    const meta = metaParts.join(' \u00b7 ')

    const cardUrl = item.url
    setTextIfChanged(state.name, item.title || item.filename || item.url.split('/').pop() || item.url)
    setAttrIfChanged(state.name, 'title', item.filename || item.title || cardUrl)

    if (state.lastMeta !== meta) {
      state.lastMeta = meta
      setAttrIfChanged(state.line, 'title', meta)
      setAttrIfChanged(state.line, 'aria-label', meta)
      patchKeyedChildren(
        state.facts,
        metaParts.map((part, index) => ({ key: `${index}:${part}`, part: String(part) })),
        entry => entry.key,
        () => el('span', 'resource-fact'),
        (node, entry) => setTextIfChanged(node, entry.part),
      )
    }

    setTextIfChanged(state.mime, itemHost)
    setAttrIfChanged(state.mime, 'title', [item.mimeType, itemHost].filter(Boolean).join(' \u00b7 '))

    if (state.copyBtn.dataset.copyUrl !== selected.url) state.copyBtn.dataset.copyUrl = selected.url
    renderCopyButton(state.copyBtn, selected.url)
    setAttrIfChanged(
      state.copyBtn,
      'title',
      itemHost ? `\u590d\u5236\u5b8c\u6574\u94fe\u63a5\uff08${itemHost}\uff09` : '\u590d\u5236\u5b8c\u6574\u94fe\u63a5',
    )

    // 1-Click Inline Quality Pills (up to 4 variants directly clickable inside the card)
    const isDrmProtected = Boolean(item.drmProtected || selected.drmProtected)
    setAttrIfChanged(article, 'data-drm', isDrmProtected ? 'true' : '')
    state.drmBadge.hidden = !isDrmProtected
    const allVariants = isDrmProtected ? [] : (item.variants || [])
    if (allVariants.length >= 2) {
      const sortedVariants = [...allVariants]
        .sort((a, b) => (b.height || 0) - (a.height || 0) || (b.bandwidth || 0) - (a.bandwidth || 0))
      const topVariants = sortedVariants.slice(0, 4).map(variant => {
        const variantResource = selectMediaVariant(item, variant)
        return {
          key: downloadSelectionKey(variantResource),
          variant,
          variantResource,
          label: compactQualityLabel(variant),
          detail: variantLabel(variant),
        }
      })
      state.qualityChips.hidden = false
      patchKeyedChildren(
        state.qualityChips,
        topVariants,
        entry => entry.key,
        entry => {
          const pill = el('button', 'quality-chip', entry.label) as HTMLButtonElement
          pill.type = 'button'
          pill.dataset.action = 'quality-pill'
          return pill
        },
        (pill, entry) => {
          const pillSendState = sending[actionKey(entry.variantResource)]
          const isSelected = selectionKey === entry.key || (!chosenVariant[item.id] && entry === topVariants[0])
          setTextIfChanged(pill, entry.label)
          setAttrIfChanged(pill, 'data-variant-key', entry.key)
          setAttrIfChanged(pill, 'data-selected', String(isSelected))
          setAttrIfChanged(pill, 'data-state', pillSendState || '')
          const pillPending = pillSendState ? SEND_PENDING.has(pillSendState) : false
          pill.disabled = pillPending
          setAttrIfChanged(pill, 'title', `${entry.detail} · 点击一键下载`)
          setAttrIfChanged(pill, 'aria-label', `一键下载 ${entry.detail}`)
          pill.onclick = () => {
            if (entry.key === downloadSelectionKey(item)) delete chosenVariant[item.id]
            else chosenVariant[item.id] = entry.key
            const currentPillState = sending[actionKey(entry.variantResource)]
            if (SEND_PENDING.has(currentPillState as SendState)) {
              renderList()
              return
            }
            void send(entry.variantResource, { forceResend: canForceResend(currentPillState) })
          }
        },
      )
      // 仅当清晰度超过 4 个时保留紧凑下拉框作完整备选
      if (allVariants.length > 4) {
        state.qualityTrigger.hidden = false
        syncSelectOptions(
          state.qualityTrigger,
          [
            { value: downloadSelectionKey(item), label: variantLabel() },
            ...allVariants.map(variant => ({
              value: downloadSelectionKey(selectMediaVariant(item, variant)),
              label: variantLabel(variant),
            })),
          ],
          selectionKey,
        )
      } else {
        state.qualityTrigger.hidden = true
      }
    } else if (allVariants.length === 1) {
      state.qualityChips.hidden = true
      state.qualityTrigger.hidden = true
    } else {
      state.qualityChips.hidden = true
      state.qualityTrigger.hidden = true
    }

    const showManualCopy = manualCopy.has(selected.url)
    state.manualCopyHelp.hidden = !showManualCopy
    state.manualCopyInput.hidden = !showManualCopy
    if (showManualCopy && state.manualCopyInput.value !== selected.url) {
      state.manualCopyInput.value = selected.url
    }

    if (isDrmProtected) {
      if (article.dataset.state !== 'drm') article.dataset.state = 'drm'
      if (state.downloadBtn.dataset.state !== 'drm') state.downloadBtn.dataset.state = 'drm'
      patchIcon(state.downloadBtn, 'close')
      setTextIfChanged(state.downloadLabel, 'DRM 保护')
      state.downloadBtn.disabled = true
      state.downloadBtn.classList.remove('busy')
      setAttrIfChanged(state.downloadBtn, 'aria-busy', 'false')
      setAttrIfChanged(state.downloadBtn, 'title', 'DRM 加密保护 · 不支持直接下载')
      state.resendBtn.hidden = true
      if (state.resendBtn.parentElement) state.resendBtn.remove()
      state.castBtn.hidden = true
      state.pushBtn.hidden = true
      return
    }

    const key = actionKey(selected)
    const sendState = sending[key]
    const urlChanged = Boolean(sentUrls[key] && sentUrls[key] !== selected.url)
    const nextStateAttr = sendState || ''
    if (article.dataset.state !== nextStateAttr) article.dataset.state = nextStateAttr
    const label = sendState ? (urlChanged && sendState === 'recoverable' ? '刷新下载' : SEND_LABELS[sendState]) : '下载'
    if (state.downloadBtn.dataset.state !== nextStateAttr) state.downloadBtn.dataset.state = nextStateAttr
    const sendPending = sendState ? SEND_PENDING.has(sendState) : false
    patchIcon(state.downloadBtn, sendPending ? 'scan' : sendState === 'queued' || sendState === 'done' ? 'check' : sendState === 'recoverable' && !urlChanged ? 'open' : 'download')
    setTextIfChanged(state.downloadLabel, label)
    const locked = sendState ? (SEND_LOCKED.has(sendState) && !urlChanged) : false
    state.downloadBtn.disabled = locked
    state.downloadBtn.classList.toggle('busy', sendPending)
    setAttrIfChanged(state.downloadBtn, 'aria-busy', String(sendPending))
    setAttrIfChanged(state.downloadBtn, 'title', sendState ? SEND_TITLES[sendState] : '发送到主程序，确认后加入下载队列')
    const showResend = canForceResend(sendState)
    state.resendBtn.hidden = !showResend
    if (showResend) {
      if (!state.resendBtn.parentElement) state.downloadBtn.after(state.resendBtn)
    } else if (state.resendBtn.parentElement) {
      state.resendBtn.remove()
    }

    // Contextual Action Hygiene: 仅对可播放流媒体/音视频展示投屏与 TVBox 按钮（未配置 TVBox 时隐藏）
    const playableMedia = isPlayableMediaResource(selected)
    state.castBtn.hidden = !playableMedia
    state.pushBtn.hidden = !playableMedia || tvboxConfigured === false
    if (playableMedia) {
      if (!state.pushBtn.hidden) {
        const pushKey = actionKey(selected, 'tvbox')
        const pushState = pushing[pushKey]
        const pushStateAttr = pushState || ''
        const pushPending = pushState === 'working' || pushState === 'awaiting'
        if (state.pushBtn.dataset.state !== pushStateAttr) state.pushBtn.dataset.state = pushStateAttr
        patchIcon(state.pushBtn, pushPending ? 'scan' : pushState === 'sent' ? 'check' : 'tv')
        setTextIfChanged(state.pushLabel, pushState ? TVBOX_PUSH_LABELS[pushState] : 'TVBox')
        state.pushBtn.disabled = pushState ? PUSH_BUSY.has(pushState) : false
        state.pushBtn.classList.toggle('busy', pushPending)
        setAttrIfChanged(state.pushBtn, 'aria-busy', String(pushPending))
      }

      const castKey = actionKey(selected, 'cast')
      const castState = pushing[castKey]
      const castStateAttr = castState || ''
      const castPending = castState === 'working' || castState === 'awaiting'
      if (state.castBtn.dataset.state !== castStateAttr) state.castBtn.dataset.state = castStateAttr
      patchIcon(state.castBtn, castPending ? 'scan' : castState === 'sent' ? 'check' : 'cast')
      setTextIfChanged(state.castLabel, castState ? CAST_PUSH_LABELS[castState] : '投屏')
      state.castBtn.disabled = castState ? PUSH_BUSY.has(castState) : false
      state.castBtn.classList.toggle('busy', castPending)
      setAttrIfChanged(state.castBtn, 'aria-busy', String(castPending))
    }
  }

  interface EmptyCardRefs {
    heading: HTMLElement
    description: HTMLSpanElement
    actionBtn: HTMLButtonElement
    actionLabel: HTMLSpanElement
    mode: 'empty' | 'filtered'
  }
  const emptyCardRefs = new WeakMap<HTMLElement, EmptyCardRefs>()

  const createEmptyCard = (): HTMLDivElement => {
    const empty = el('div', 'empty')
    const emptyIcon = el('div', 'empty-icon')
    emptyIcon.append(icon('media'))
    const heading = el('strong')
    const description = el('span')
    const actionBtn = el('button', 'empty-retry') as HTMLButtonElement
    actionBtn.type = 'button'
    const actionLabel = el('span', '', '嗅探全部资源')
    actionBtn.append(icon('scan'), actionLabel)
    empty.append(emptyIcon, heading, description, actionBtn)
    const refs: EmptyCardRefs = { heading, description, actionBtn, actionLabel, mode: 'empty' }
    emptyCardRefs.set(empty, refs)
    actionBtn.addEventListener('click', () => {
      if (refs.mode === 'filtered') {
        filterCategory = 'all'
        filterQuery = ''
        filterSearch.value = ''
        renderList()
      } else {
        void rescanPage()
      }
    })
    return empty
  }

  const renderList = () => {
    const visible = visibleMediaResources(showCandidates ? resources : resources.filter(resource => isDefaultPopupResource(resource)))
    setTextIfChanged(count, String(visible.length))
    quickDrawer.hidden = activePanelIndex !== 0
    const categoryCounts = countPopupCategories(visible)
    if (filterCategory !== 'all' && categoryCounts[filterCategory] === 0) {
      filterCategory = 'all'
    }
    const filtered = filterPopupResources(visible, filterCategory, filterQuery)
    const showToolbar = visible.length > 1 || filterCategory !== 'all' || filterQuery.trim() !== ''
    resourceToolbar.hidden = !showToolbar
    if (showToolbar) {
      const chipDefs: Array<[ResourceFilterCategory, string]> = [
        ['all', '全部'],
        ['stream', '流媒体'],
        ['video', '视频'],
        ['audio', '音频'],
        ['file', '文件'],
      ]
      const activeNonAllCategories = chipDefs.slice(1).filter(([key]) => categoryCounts[key] > 0)
      filterChips.hidden = activeNonAllCategories.length < 2
      const visibleChips = filterChips.hidden
        ? []
        : chipDefs.filter(([key]) => key === 'all' || categoryCounts[key] > 0)
      patchKeyedChildren(
        filterChips,
        visibleChips,
        ([key]) => key,
        ([key, label]) => {
          const chip = el('button', 'filter-chip') as HTMLButtonElement
          chip.type = 'button'
          chip.append(el('span', '', label), el('b', '', '0'))
          chip.addEventListener('click', () => {
            filterCategory = key
            renderList()
          })
          return chip
        },
        (chip, [key, label]) => {
          setAttrIfChanged(chip, 'aria-pressed', String(filterCategory === key))
          const nameSpan = chip.querySelector('span')
          const countEl = chip.querySelector('b')
          if (nameSpan) setTextIfChanged(nameSpan, label)
          if (countEl) setTextIfChanged(countEl, String(categoryCounts[key]))
        },
      )
      const batchTargets = filtered
        .filter(item => !item.drmProtected)
        .map(resolveSelectedVariant)
        .filter(candidate => !candidate.drmProtected && sendClickAction(sending[actionKey(candidate)]) === 'send')
      const batchCount = Math.min(batchTargets.length, 20)
      batchBtn.disabled = batchRunning || batchCount === 0
      const batchLabel = batchRunning
        ? '批量发送中…'
        : filtered.length === 0
          ? '批量下载 (0)'
          : batchCount > 0
            ? `批量下载 (${batchCount})`
            : '已全部发送'
      patchIcon(batchBtn, batchRunning ? 'scan' : 'download')
      setTextIfChanged(batchBtnLabel, batchLabel)
      batchBtn.onclick = () => {
        if (batchRunning || batchCount === 0) return
        void sendBatch(batchTargets.slice(0, 20))
      }
    }
    const cookieAuthorized = Boolean(host && authorizedCookieHosts.includes(host))
    const cookieDowngraded = Boolean(host && cookieDowngradedHosts.includes(host))
    const hasAuthSensitiveMedia = visible.some(
      item => !item.drmProtected && (item.kind === 'hls' || item.kind === 'dash' || item.kind === 'media' || Boolean(item.audioUrl)),
    )
    const showCookieBanner = Boolean(
      host
      && !cookieAuthorized
      && (cookieDowngraded || (siteHasCookies && hasAuthSensitiveMedia)),
    )
    if (showCookieBanner) {
      setTextIfChanged(
        cookieAuthText,
        cookieDowngraded
          ? '刚才的下载需携带本站登录态 · 授权后启用多线程加速'
          : '检测到本站登录态 · 高画质或受限媒体可能需授权 Cookie',
      )
    }
    cookieAuthBanner.hidden = !showCookieBanner
    cookieAuthQuickBtn.disabled = Boolean(settingsBusy)
    if (!visible.length) {
      const heading = resourceState === 'loading'
        ? '\u6b63\u5728\u8bfb\u53d6\u5f53\u524d\u9875\u9762'
        : resourceState === 'scanning'
          ? '\u6b63\u5728嗅探全部资源'
          : resourceState === 'error'
            ? '\u672a\u80fd\u5b8c\u6210\u8bc6\u522b'
            : !canScanPage ? '此页面不支持媒体识别' : '\u5f53\u524d\u9875\u9762\u8fd8\u6ca1\u6709\u53ef\u4e0b\u8f7d\u8d44\u6e90'
      const description = resourceState === 'loading' || resourceState === 'scanning'
        ? '\u6b63\u5728\u68c0\u67e5\u89c6\u9891\u3001\u97f3\u9891\u548c\u5a92\u4f53\u6e05\u5355\u3002'
        : !canScanPage ? '请切换到普通网页，播放媒体后再打开插件。' : '\u5148\u64ad\u653e\u9875\u9762\u4e2d\u7684\u5a92\u4f53\uff0c\u6216\u70b9\u51fb\u4e0b\u65b9嗅探全部资源\u3002'
      const showRetry = canScanPage && resourceState !== 'loading' && resourceState !== 'scanning'
      patchKeyedChildren(
        list,
        [{ mode: 'empty' as const, heading, description, showRetry }],
        () => '__empty__',
        createEmptyCard,
        (empty, spec) => {
          const refs = emptyCardRefs.get(empty)
          if (!refs) return
          refs.mode = spec.mode
          setTextIfChanged(refs.heading, spec.heading)
          setTextIfChanged(refs.description, spec.description)
          refs.actionBtn.hidden = !spec.showRetry
          setTextIfChanged(refs.actionLabel, '嗅探全部资源')
        },
      )
      return
    }
    if (!filtered.length) {
      patchKeyedChildren(
        list,
        [{ mode: 'filtered' as const }],
        () => '__empty__',
        createEmptyCard,
        empty => {
          const refs = emptyCardRefs.get(empty)
          if (!refs) return
          refs.mode = 'filtered'
          setTextIfChanged(refs.heading, '没有匹配当前筛选的资源')
          setTextIfChanged(refs.description, '试试清空关键词或切回“全部”类型。')
          refs.actionBtn.hidden = false
          setTextIfChanged(refs.actionLabel, '重置筛选')
        },
      )
      return
    }
    patchKeyedChildren(
      list,
      filtered,
      item => resourceFingerprint(item),
      createResourceCard,
      updateResourceCard,
    )
  }

  syncEnableAriaLabels = () => {
    const enableStateLabel = enabled ? '已开启' : '已关闭'
    // Keep hidden panel-2 button non-colliding while panel-0 is active so XPath
    // `starts-with(@aria-label, '自动接管')` always resolves to the visible button.
    enableBtn.setAttribute(
      'aria-label',
      activePanelIndex === 2 ? `自动接管：${enableStateLabel}` : `全局自动接管：${enableStateLabel}`,
    )
    quickEnablePill.setAttribute(
      'aria-label',
      activePanelIndex === 0 ? `自动接管：${enabled ? '开' : '关'}` : `快捷自动接管：${enabled ? '开' : '关'}`,
    )
  }

  const refreshButtons = () => {
    enableBtn.textContent = enabled ? '已开启' : '已关闭'
    syncEnableAriaLabels()
    enableBtn.title = enabled ? '全局下载接管已开启；点击关闭' : '全局下载接管已关闭；点击开启'
    enableBtn.setAttribute('aria-pressed', String(enabled))
    enableBtn.classList.toggle('active', enabled)
    quickDownloadBtn.textContent = quickDownloadEnabled ? '已开启' : '已关闭'
    quickDownloadBtn.setAttribute('aria-label', `静默快速下载（不弹确认框）：${quickDownloadBtn.textContent}`)
    quickDownloadBtn.title = quickDownloadEnabled
      ? '静默快速下载已开启：接管下载后直接开始，不弹确认框；点击关闭'
      : '静默快速下载已关闭：接管下载后显示确认框；点击开启'
    quickDownloadBtn.setAttribute('aria-pressed', String(quickDownloadEnabled))
    quickDownloadBtn.classList.toggle('active', quickDownloadEnabled)
    speedLimitBtn.textContent = speedLimitEnabled ? '已开启' : '已关闭'
    speedLimitBtn.setAttribute('aria-label', `下载限速：${speedLimitBtn.textContent}`)
    speedLimitBtn.title = speedLimitEnabled
      ? (speedLimitKib > 0 ? `全局下载限速已开启（${speedLimitKib} KiB/s）；点击关闭` : '全局下载限速已开启；点击关闭')
      : '全局下载限速已关闭；点击开启'
    speedLimitBtn.setAttribute('aria-pressed', String(speedLimitEnabled))
    speedLimitBtn.classList.toggle('active', speedLimitEnabled)
    setTextIfChanged(
      speedLimitDesc,
      speedLimitEnabled && speedLimitKib > 0
        ? `当前限速 ${speedLimitKib} KiB/s，与主程序实时同步。`
        : '全局限速开关，与主程序限速状态保持同步。',
    )

    // 同步底部常驻单行快捷条状态
    quickEnablePill.setAttribute('aria-pressed', String(enabled))
    setTextIfChanged(quickEnablePill, `自动接管: ${enabled ? '开' : '关'}`)
    quickSilentPill.setAttribute('aria-pressed', String(quickDownloadEnabled))
    quickSilentPill.setAttribute('aria-label', `静默下载：${quickDownloadEnabled ? '开' : '关'}`)
    setTextIfChanged(quickSilentPill, `静默下载: ${quickDownloadEnabled ? '开' : '关'}`)
    quickSpeedPill.setAttribute('aria-pressed', String(speedLimitEnabled))
    quickSpeedPill.setAttribute('aria-label', `下载限速：${speedLimitEnabled ? '开' : '关'}`)
    setTextIfChanged(quickSpeedPill, speedLimitEnabled && speedLimitKib > 0 ? `限速: ${speedLimitKib}K` : `限速: ${speedLimitEnabled ? '开' : '关'}`)

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
    const suppressedKinds = suppressions
      .filter(rule => rule.host === suppressionHost || (host && rule.host === host))
      .map(rule => rule.kind)
    restorePromptsBtn.hidden = suppressedKinds.length === 0
    restorePromptsBtn.title = suppressedKinds.length
      ? `\u6062\u590d\u672c\u7ad9 ${suppressedKinds.join('\u3001')} \u8d44\u6e90\u7684\u81ea\u52a8\u63d0\u793a`
      : ''
    for (const button of [enableBtn, quickDownloadBtn, speedLimitBtn, cookieBtn, excludeBtn, restorePromptsBtn]) {
      const globalSetting = button === enableBtn || button === quickDownloadBtn || button === speedLimitBtn
      button.disabled = Boolean(settingsBusy)
        || (button === restorePromptsBtn ? (!host && !suppressionHost) : (!globalSetting && !host))
      button.setAttribute('aria-busy', String(settingsBusy === button))
    }
    const quickDisabled = Boolean(settingsBusy)
    quickEnablePill.disabled = quickDisabled
    quickSilentPill.disabled = quickDisabled
    quickSpeedPill.disabled = quickDisabled
    renderConnectionStatus()
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
    const key = actionKey(item, 'tvbox')
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
    const key = actionKey(item, 'cast')
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

  const copyLink = async (url: string) => {
    if (copyStates.get(url) === 'copying') return
    clearTimeout(copyResetTimers.get(url))
    copyResetTimers.delete(url)
    setCopyState(url, 'copying')
    try {
      let copied = false
      try {
        await navigator.clipboard.writeText(url)
        copied = true
      } catch {}
      if (!copied) {
        const focused = document.activeElement
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
          if (focused instanceof HTMLElement && focused.isConnected) focused.focus({ preventScroll: true })
        }
      }
      manualCopy.delete(url)
      for (const button of list.querySelectorAll<HTMLButtonElement>('[data-action=copy]')) {
        if (button.dataset.copyUrl !== url) continue
        const card = button.closest('article')
        const input = card?.querySelector<HTMLInputElement>('.manual-copy')
        const help = card?.querySelector<HTMLElement>('.manual-copy-help')
        if (input) input.hidden = true
        if (help) help.hidden = true
      }
      setCopyState(url, 'copied')
      copyResetTimers.set(url, setTimeout(() => {
        setCopyState(url)
        copyResetTimers.delete(url)
      }, 1_800))
    } catch {
      setCopyState(url)
      manualCopy.add(url)
      renderList()
      const card = [...list.querySelectorAll('article')].find(card => card.querySelector<HTMLInputElement>('.manual-copy')?.value === url)
      card?.querySelector<HTMLInputElement>('.manual-copy')?.focus({ preventScroll: true })
    }
  }

  const confirmDownload = async (item: MediaResource, handoffId: string, options?: { preserveError?: boolean }): Promise<string | null> => {
    const key = actionKey(item)
    sending[key] = 'confirming'
    renderList()
    let accepted: boolean
    try {
      accepted = await waitForDownloadConfirmation(message => browser.runtime.sendMessage(message), handoffId)
    } catch (reason) {
      sending[key] = 'checking'
      const message = reason instanceof Error ? reason.message : '无法读取下载确认状态'
      if (!options?.preserveError) setError(message)
      renderList()
      return message
    }
    sending[key] = accepted ? 'queued' : 'canceled'
    renderList()
    return null
  }

  const send = async (item: MediaResource, options?: { preserveError?: boolean; forceResend?: boolean }): Promise<string | null> => {
    const key = actionKey(item)
    if (!options?.forceResend && sending[key] && SEND_LOCKED.has(sending[key])) return null
    if (sending[key] && SEND_PENDING.has(sending[key])) return null
    if (!options?.preserveError) setError('')
    sending[key] = 'sending'
    sentUrls[key] = item.url
    renderList()
    let delivered = false
    try {
      const response = await withDeadline(
        browser.runtime.sendMessage({
          type: 'download-now',
          resource: item,
          ...(options?.forceResend ? { forceResend: true } : {}),
        }),
        15_000,
        '下载器响应超时，正在按原请求核对',
      )
      delivered = true
      if (!response?.ok && response?.ownership_uncertain) {
        sending[key] = 'checking'
        const message = response.error || '暂时无法确认交接状态'
        if (!options?.preserveError) setError(message)
        renderList()
        return message
      }
      if (!response?.ok) throw new Error(response?.error || '桌面端未接受下载请求')
      // Sending offer directly wakes Core from user_shutdown; refresh header badge if needed.
      if (connectionState !== 'online') void recheckConnection()
      const handoff = response.handoff || {}
      const state = sendStateFromHandoff({
        status: handoff.status,
        presentation: handoff.presentation,
        taskStatus: handoff.task_status,
        taskErrorCode: handoff.task_error_code,
      })
      if (state === 'confirming') return await confirmDownload(item, String(handoff.id || ''), options)
      sending[key] = state
      renderList()
      return null
    } catch (reason) {
      sending[key] = delivered ? 'retry' : 'checking'
      const message = reason instanceof Error ? reason.message : '\u53d1\u9001\u5230\u684c\u9762\u7aef\u5931\u8d25'
      if (!options?.preserveError) setError(message)
      renderList()
      return message
    }
  }

  const sendBatch = async (targets: MediaResource[]) => {
    if (batchRunning || !targets.length) return
    const eligible = targets.filter(target => sendClickAction(sending[actionKey(target)]) === 'send')
    if (!eligible.length) return
    batchRunning = true
    setError('')
    for (const target of eligible) {
      const key = actionKey(target)
      sending[key] = 'sending'
      sentUrls[key] = target.url
    }
    renderList()
    try {
      // 一次性交给后台 Service Worker 串行投递，避免第 1 项唤醒桌面确认框导致 Popup 关闭后丢失剩余任务。
      const response = await withDeadline(
        browser.runtime.sendMessage({ type: 'download-batch', resources: eligible }),
        30_000,
        '批量发送等待超时，后台仍在继续处理',
      )
      if (connectionState !== 'online' && Number(response?.sentCount) > 0) void recheckConnection()
      const results = Array.isArray(response?.results) ? response.results : []
      const bySelection = new Map<string, any>(results.map((entry: any) => [`${entry.id}:${entry.selectionKey}`, entry]))
      const confirmations: Array<Promise<string | null>> = []
      for (const target of eligible) {
        const key = actionKey(target)
        const entry = bySelection.get(`${target.id}:${downloadSelectionKey(target)}`)
        if (!entry) {
          sending[key] = 'checking'
          continue
        }
        if (!entry.ok) {
          sending[key] = entry.ownership_uncertain ? 'checking' : 'retry'
          continue
        }
        const handoff = entry.handoff || {}
        const nextState = sendStateFromHandoff({
          status: handoff.status,
          presentation: handoff.presentation,
          taskStatus: handoff.task_status,
          taskErrorCode: handoff.task_error_code,
        })
        if (nextState === 'confirming' && handoff.id) {
          confirmations.push(confirmDownload(target, String(handoff.id), { preserveError: true }))
        } else {
          sending[key] = nextState
        }
      }
      renderList()
      const failedCount = Number(response?.failedCount || 0)
      const sentCount = Number(response?.sentCount || 0)
      if (failedCount > 0) {
        const errDetail = String(response?.error || '部分资源发送失败')
        setError(sentCount > 0 ? `已发送 ${sentCount} 项，${failedCount} 项失败：${errDetail}` : errDetail)
      }
      if (confirmations.length) void Promise.allSettled(confirmations)
    } catch (reason) {
      for (const target of eligible) {
        const key = actionKey(target)
        if (sending[key] === 'sending') sending[key] = 'checking'
      }
      setError(reason instanceof Error ? reason.message : '批量发送到桌面端失败')
    } finally {
      batchRunning = false
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
  quickDownloadBtn.addEventListener('click', async () => {
    if (settingsBusy) return
    const requested = !quickDownloadEnabled
    settingsBusy = quickDownloadBtn
    refreshButtons()
    setError('')
    try {
      const response = await withDeadline(
        browser.runtime.sendMessage({ type: 'set-takeover-settings', quickDownloadEnabled: requested }),
        10_000,
        '保存快速下载设置超时，请重试',
      )
      if (!response?.ok) {
        setError(response?.error || '保存快速下载设置失败')
        return
      }
      if (typeof response.quick_download_enabled !== 'boolean') {
        setError('桌面端未返回有效的快速下载设置')
        return
      }
      quickDownloadEnabled = response.quick_download_enabled
      if (quickDownloadEnabled !== requested) setError('快速下载设置已被桌面端的更新值覆盖')
      refreshButtons()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '保存快速下载设置失败')
    } finally {
      settingsBusy = undefined
      refreshButtons()
    }
  })
  speedLimitBtn.addEventListener('click', async () => {
    if (settingsBusy) return
    const requested = !speedLimitEnabled
    settingsBusy = speedLimitBtn
    refreshButtons()
    setError('')
    try {
      const response = await withDeadline(
        browser.runtime.sendMessage({ type: 'set-takeover-settings', speedLimitEnabled: requested }),
        10_000,
        '保存限速设置超时，请重试',
      )
      if (!response?.ok) {
        setError(response?.error || '保存限速设置失败')
        return
      }
      if (typeof response.speed_limit_enabled !== 'boolean') {
        setError('桌面端未返回有效的限速设置')
        return
      }
      speedLimitEnabled = response.speed_limit_enabled
      if (Number.isFinite(Number(response.speed_limit_kib)) && Number(response.speed_limit_kib) >= 0) {
        speedLimitKib = Math.floor(Number(response.speed_limit_kib))
      }
      if (speedLimitEnabled !== requested) setError('限速设置已被桌面端的更新值覆盖')
      refreshButtons()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '保存限速设置失败')
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
      if (next.includes(host) && cookieDowngradedHosts.includes(host)) {
        cookieDowngradedHosts = cookieDowngradedHosts.filter(entry => entry !== host)
        void browser.storage?.session?.set?.({ [COOKIE_DOWNGRADED_HOSTS_KEY]: cookieDowngradedHosts }).catch(() => undefined)
      }
      refreshButtons()
      renderList()
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
    if ((!host && !suppressionHost) || settingsBusy) return
    const next = suppressions.filter(rule => rule.host !== suppressionHost && rule.host !== host)
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
  const extensionOrigin = browser.runtime.getURL('')
  const activeTab = windowTabs.find(candidate => candidate.active)
  const tab = windowTabs.find(candidate => candidate.active && /^https?:\/\//i.test(candidate.url || ''))
    || (activeTab?.url?.startsWith(extensionOrigin)
      ? [...windowTabs].reverse().find(candidate => /^https?:\/\//i.test(candidate.url || ''))
      : undefined)
    || activeTab
  const pageUrl = tab?.url || ''
  pageTabId = /^https?:\/\//i.test(pageUrl) ? tab?.id : undefined
  if (tabs[1].getAttribute('aria-selected') === 'true') selectPanel(1)
  if (pageTabId === undefined) subtitlePanel.replaceChildren(el('p', 'panel-hint', '请在包含播放器的网页中使用字幕翻译。'))
  host = normalizeHost(pageUrl)
  suppressionHost = hostForPage(pageUrl)
  if (host && /^https?:\/\//i.test(pageUrl) && browser.cookies?.getAll) {
    void browser.cookies.getAll({ url: pageUrl }).then(cookies => {
      const nextHas = Array.isArray(cookies) && cookies.length > 0
      if (siteHasCookies !== nextHas) {
        siteHasCookies = nextHas
        renderList()
      }
    }).catch(() => undefined)
  }
  settingsSite.textContent = host ? `当前站点 · ${host}` : '当前页面不是普通网页，站点设置不可用。'
  title.textContent = host || '当前页面不支持媒体识别'
  title.title = pageUrl
  const downloadErrorKey = `download-error:${tab?.id}`
  void browser.storage.session.get([downloadErrorKey, COOKIE_DOWNGRADED_HOSTS_KEY]).then(stored => {
    if (stored[downloadErrorKey]) setError(String(stored[downloadErrorKey]))
    if (Array.isArray(stored[COOKIE_DOWNGRADED_HOSTS_KEY])) {
      cookieDowngradedHosts = stored[COOKIE_DOWNGRADED_HOSTS_KEY].map(entry => normalizeHost(String(entry || ''))).filter(Boolean)
      renderList()
    }
  }).catch(() => undefined)
  browser.storage.onChanged.addListener((changes, area) => {
    if (area === 'session') {
      if (changes[downloadErrorKey]) setError(String(changes[downloadErrorKey].newValue || ''))
      if (changes[COOKIE_DOWNGRADED_HOSTS_KEY]) {
        const raw = changes[COOKIE_DOWNGRADED_HOSTS_KEY].newValue
        cookieDowngradedHosts = Array.isArray(raw)
          ? raw.map(entry => normalizeHost(String(entry || ''))).filter(Boolean)
          : []
        renderList()
      }
    }
    if (area === 'local' && !settingsBusy) {
      let dirty = false
      if (changes.enabled) {
        enabled = changes.enabled.newValue !== false
        dirty = true
      }
      if (changes.quickDownloadEnabled) {
        quickDownloadEnabled = changes.quickDownloadEnabled.newValue === true
        dirty = true
      }
      if (changes.speedLimitEnabled) {
        speedLimitEnabled = changes.speedLimitEnabled.newValue === true
        dirty = true
      }
      if (changes.speedLimitKib) {
        const nextKib = Number(changes.speedLimitKib.newValue || 0)
        speedLimitKib = Number.isFinite(nextKib) && nextKib > 0 ? Math.floor(nextKib) : 0
        dirty = true
      }
      if ('tvboxConfigured' in changes) {
        tvboxConfigured = typeof changes.tvboxConfigured?.newValue === 'boolean'
          ? changes.tvboxConfigured.newValue
          : undefined
        renderList()
      }
      if (dirty) refreshButtons()
    }
  })
  canScanPage = Number.isInteger(tab?.id) && /^https?:\/\//i.test(pageUrl)
  let listGeneration = 0
  rescanPage = async () => {
    if (!canScanPage || tab?.id == null || resourceState === 'scanning') return
    listGeneration += 1
    const scanGeneration = listGeneration
    showCandidates = true
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
      if (scanGeneration === listGeneration) {
        resources = Array.isArray(listed) ? listed : []
        resourceState = 'ready'
      }
    } catch (reason) {
      if (scanGeneration === listGeneration) {
        resourceState = 'error'
        setError(reason instanceof Error ? reason.message : '嗅探全部资源\u5931\u8d25')
      }
    } finally {
      scanBtn.disabled = !canScanPage
      renderList()
    }
  }
  scanBtn.disabled = !canScanPage
  scanBtn.addEventListener('click', () => void rescanPage())
  renderList()

  const stored = await withDeadline(browser.storage.local.get([
    'enabled', 'quickDownloadEnabled', 'speedLimitEnabled', 'speedLimitKib', 'tvboxConfigured',
    'excludedHosts', 'authorizedCookieHosts', HANDOFF_SUPPRESSION_STORAGE_KEY,
  ]), 1_000).catch(() => ({} as Record<string, unknown>))
  enabled = stored.enabled !== false
  quickDownloadEnabled = stored.quickDownloadEnabled === true
  speedLimitEnabled = stored.speedLimitEnabled === true
  if (typeof stored.tvboxConfigured === 'boolean') tvboxConfigured = stored.tvboxConfigured
  const storedLimitKib = Number(stored.speedLimitKib || 0)
  speedLimitKib = Number.isFinite(storedLimitKib) && storedLimitKib > 0 ? Math.floor(storedLimitKib) : 0
  authorizedCookieHosts = normalizeCookiePermissionHosts(stored.authorizedCookieHosts)
  excluded = Array.isArray(stored.excludedHosts)
    ? stored.excludedHosts.map(value => normalizeHost(String(value || ''))).filter(Boolean)
    : []
  suppressions = normalizeHandoffSuppressions(stored[HANDOFF_SUPPRESSION_STORAGE_KEY])
  enableBtn.disabled = false
  quickDownloadBtn.disabled = false
  speedLimitBtn.disabled = false
  refreshButtons()

  function showConnection(response: any) {
    connectionChecked = true
    connectionReconnecting = Boolean(response?.reconnecting) && !response?.ok
    connectionState = classifyEngineConnectionState(response)
    if (!settingsBusy) {
      if (typeof response?.takeover_enabled === 'boolean') enabled = response.takeover_enabled
      const remoteQuick = typeof response?.quick_download_enabled === 'boolean'
        ? response.quick_download_enabled
        : typeof response?.browser_quick_download_enabled === 'boolean'
          ? response.browser_quick_download_enabled
          : undefined
      if (remoteQuick !== undefined) quickDownloadEnabled = remoteQuick
      const remoteSpeedLimit = typeof response?.speed_limit_enabled === 'boolean'
        ? response.speed_limit_enabled
        : typeof response?.download_speed_limit_enabled === 'boolean'
          ? response.download_speed_limit_enabled
          : undefined
      if (remoteSpeedLimit !== undefined) speedLimitEnabled = remoteSpeedLimit
      const remoteSpeedKib = Number(response?.speed_limit_kib ?? response?.download_speed_limit_kib)
      if (Number.isFinite(remoteSpeedKib) && remoteSpeedKib >= 0) speedLimitKib = Math.floor(remoteSpeedKib)
    }
    const remoteTvbox = typeof response?.tvbox_configured === 'boolean'
      ? response.tvbox_configured
      : typeof response?.tvboxConfigured === 'boolean'
        ? response.tvboxConfigured
        : typeof response?.tvbox_endpoint === 'string'
          ? response.tvbox_endpoint.trim().length > 0
          : undefined
    if (remoteTvbox !== undefined && remoteTvbox !== tvboxConfigured) {
      tvboxConfigured = remoteTvbox
      renderList()
    }
    refreshButtons()
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

  recheckConnection = async () => {
    connectionChecked = true
    connectionReconnecting = true
    renderConnectionStatus()
    const reply = await withDeadline(browser.runtime.sendMessage({ type: 'ping' }), 2_500)
      .catch(error => ({ ok: false, error: error instanceof Error ? error.message : String(error || '') }))
    showConnection(reply)
  }

  const connectionRequest = browser.runtime.sendMessage({ type: 'ping' })
  const [listed, connection] = await Promise.all([
    withDeadline(browser.runtime.sendMessage({ type: 'list', pageUrl, tabId: tab?.id }), 1_500).catch(() => []),
    withDeadline(connectionRequest, 1_800).catch(() => ({ reconnecting: true })),
  ])
  if (listGeneration === 0) {
    resources = Array.isArray(listed) ? listed : []
    resourceState = 'ready'
  }
  showConnection(connection)
  if (connection?.reconnecting) void connectionRequest.then(showConnection).catch(() => showConnection({ ok: false }))
  refreshButtons()
  renderList()
  document.documentElement.dataset.popupReady = 'ready'

  let transfersPolling = false
  async function refreshBrowserTransfers() {
    if (transfersPolling || document.hidden) return
    transfersPolling = true
    try {
      const transfers = await browser.runtime.sendMessage({ type: 'local-download-progress', tabId: tab?.id })
      const items = Array.isArray(transfers) ? transfers : []
      browserTransfers.hidden = !items.length
      const labels: Record<string, string> = {
        receiving: '正在由浏览器接收，完成后导入', paused: '浏览器已暂停，恢复后继续接收',
        interrupted: '浏览器接收中断，源文件保留，请在浏览器恢复',
        importing: '浏览器接收完成，正在等待桌面确认或导入', recovering: '正在恢复交接，源文件保留',
      }
      patchKeyedChildren(
        browserTransfers,
        items.map((transfer, index) => ({
          key: String(transfer.id ?? transfer.downloadId ?? `${transfer.filename || 'transfer'}:${index}`),
          transfer,
        })),
        entry => entry.key,
        () => {
          const row = el('div', 'browser-transfer')
          row.append(el('strong'), el('span', 'transfer-state'), el('span', 'transfer-bytes'))
          return row
        },
        (row, { transfer }) => {
          const titleEl = row.querySelector('strong')
          const stateEl = row.querySelector('.transfer-state')
          const bytesEl = row.querySelector('.transfer-bytes')
          if (titleEl) setTextIfChanged(titleEl, String(transfer.filename || '浏览器下载'))
          if (stateEl) setTextIfChanged(stateEl, labels[String(transfer.state)] || '正在恢复交接')
          if (bytesEl) {
            setTextIfChanged(
              bytesEl,
              `${formatBytes(Number(transfer.bytesReceived) || 0)}${Number(transfer.totalBytes) > 0 ? ` / ${formatBytes(Number(transfer.totalBytes))}` : ''}`,
            )
          }
        },
      )
    } catch {}
    finally { transfersPolling = false }
  }
  void refreshBrowserTransfers()
  const transfersTimer = window.setInterval(() => void refreshBrowserTransfers(), 1000)
  window.addEventListener('pagehide', () => clearInterval(transfersTimer), { once: true })
  void browser.runtime.sendMessage({ type: 'download-states', tabId: tab?.id }).then(pending => {
    if (!Array.isArray(pending)) return
    for (const handoff of pending) {
      const item = resources.find(resource => resource.id === handoff.resourceId)
      if (!item) continue
      const selected = handoff.selectionKey
        ? [item, ...(item.variants || []).map(variant => selectMediaVariant(item, variant))]
          .find(candidate => downloadSelectionKey(candidate) === handoff.selectionKey)
        : item
      if (!selected) continue
      const key = actionKey(selected)
      if (sending[key]) continue
      if (typeof handoff.url === 'string' && handoff.url) sentUrls[key] = handoff.url
      if (selected !== item && !chosenVariant[item.id]) chosenVariant[item.id] = downloadSelectionKey(selected)
      const restored = sendStateFromHandoff(handoff)
      if (restored !== 'confirming') {
        sending[key] = restored
        renderList()
        continue
      }
      void confirmDownload(selected, String(handoff.id)).catch(reason => {
        sending[key] = 'retry'
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
