import { browser } from 'wxt/browser'
import { compactResources, pageResourceKey, pruneExpiredResources, type MediaResource } from './resources'
import { SUBTITLE_LANGUAGES, subtitlePage, subtitleSite } from './subtitleSettings'
import { withDeadline } from './asyncDeadline'

export function mountSubtitlePopup(root: HTMLElement, tabId: number | undefined): void {
  root.className = 'subtitle-panel'
  root.dataset.subtitlePanel = 'true'
  const node = <K extends keyof HTMLElementTagNameMap>(tag: K, text = '') => {
    const value = document.createElement(tag); value.textContent = text; return value
  }
  const heading = node('div'); heading.className = 'subtitle-heading'
  heading.append(node('strong', '在线字幕翻译'))
  const status = node('span', '读取状态中…'); status.setAttribute('role', 'status'); heading.append(status)
  const error = node('p'); error.className = 'subtitle-error'; error.hidden = true
  const row = node('div'); row.className = 'subtitle-row'
  const enabled = node('input'); enabled.type = 'checkbox'; enabled.dataset.subtitleEnabled = 'true'
  const enabledLabel = node('label'); enabledLabel.append(enabled, node('span', '全局开启'))
  const site = node('select'); site.setAttribute('aria-label', '本站字幕翻译')
  const page = node('select'); page.setAttribute('aria-label', '本页字幕翻译')
  for (const [value, label] of [['inherit', '默认'], ['on', '开启'], ['off', '关闭']]) {
    site.append(new Option(`本站：${label}`, value)); page.append(new Option(`本页：${label}`, value))
  }
  row.append(enabledLabel)
  const advanced = node('details'); advanced.append(node('summary', '字幕与站点选项'))
  const scopeRow = node('div'); scopeRow.className = 'subtitle-row'; scopeRow.append(site, page)
  const languageRow = node('div'); languageRow.className = 'subtitle-row'
  const source = node('select'); source.setAttribute('aria-label', '来源语言'); source.append(new Option('自动识别', 'auto'))
  const target = node('select'); target.setAttribute('aria-label', '目标语言')
  for (const [code, label] of SUBTITLE_LANGUAGES) { source.append(new Option(label, code)); target.append(new Option(label, code)) }
  const offset = node('input'); offset.type = 'number'; offset.step = '0.2'; offset.min = '-30'; offset.max = '30'; offset.setAttribute('aria-label', '字幕时间偏移秒')
  languageRow.append(source, node('span', '→'), target)
  const originals = node('input'); originals.type = 'checkbox'
  const originalLabel = node('label'); originalLabel.append(originals, node('span', '显示原文'))
  const prefetch = node('input'); prefetch.type = 'checkbox'; prefetch.dataset.subtitlePrefetch = 'true'
  const prefetchLabel = node('label'); prefetchLabel.append(prefetch, node('span', '同步字幕（不足时暂停视频）'))
  const prefetchHelp = node('small', '关闭：实时翻译，不控制视频暂停，译文会有延迟。开启：先等待字幕，再播放；字幕不足会再次暂停，倍速播放可能频繁等待。仅支持已确认音源的点播。')
  const offsetRow = node('div'); offsetRow.className = 'subtitle-row'; offsetRow.append(node('span', '字幕偏移'), offset, node('span', '秒'))
  const player = node('select'); player.setAttribute('aria-label', '字幕播放器'); player.className = 'subtitle-player'
  const start = node('button', '开始翻译'); start.className = 'hlsd-button primary'; start.dataset.subtitleStart = 'true'
  const actionRow = node('div'); actionRow.className = 'subtitle-row'; actionRow.append(player, start)
  const tracks = node('small')
  const provider = node('details'); provider.append(node('summary', '翻译服务设置'))
  const form = node('form'); form.className = 'subtitle-provider'
  const service = node('select'); service.setAttribute('aria-label', '翻译服务')
  service.append(new Option('阿里云百炼', 'dashscope'), new Option('QwenCloud 实时翻译', 'qwencloud'))
  const workspace = node('input'); workspace.placeholder = '百炼业务空间 ID'; workspace.setAttribute('aria-label', '百炼业务空间 ID')
  const region = node('select'); region.setAttribute('aria-label', '翻译服务地域')
  region.append(new Option('华北 2（北京）', 'cn-beijing'), new Option('新加坡', 'ap-southeast-1'))
  const dashscopeFields = node('div'); dashscopeFields.className = 'subtitle-provider-fields'; dashscopeFields.append(workspace, region)
  const key = node('input'); key.type = 'password'; key.autocomplete = 'off'; key.placeholder = 'API Key'; key.setAttribute('aria-label', '字幕服务 API Key')
  const save = node('button', '保存服务设置'); save.type = 'submit'; save.className = 'hlsd-button'
  const clear = node('button', '清除 Key'); clear.type = 'button'; clear.className = 'hlsd-button subtle'
  const keyState = node('small', '正在读取服务配置…')
  const keyActions = node('div'); keyActions.className = 'subtitle-row'; keyActions.append(save, clear)
  const providerHelp = node('small')
  form.append(service, dashscopeFields, key, keyActions, keyState, providerHelp)
  const updateProviderFields = () => {
    const dashscope = service.value === 'dashscope'
    dashscopeFields.hidden = !dashscope
    workspace.required = dashscope
    providerHelp.textContent = `${dashscope ? '百炼需要业务空间 ID 和地域。' : 'QwenCloud 使用固定官方实时翻译服务。'}点击开始后上传采集的网页音频；开启预翻译后会提前读取当前音轨。API Key 由桌面端加密保存。`
  }
  service.onchange = updateProviderFields
  updateProviderFields()
  provider.append(form)
  advanced.append(originalLabel, offsetRow, scopeRow, tracks)
  root.append(heading, row, languageRow, prefetchLabel, prefetchHelp, actionRow, error, advanced, provider)
  let state: any
  let busy = false
  let refreshing = false
  let disposed = false
  let resourceKey = ''
  let resourceGeneration = 0
  let resources: MediaResource[] = []
  let live = false
  let hasKey = false
  let loadingProvider = true
  const updateControls = () => {
    for (const control of [enabled, site, page, source, target, offset, originals, prefetch, player]) control.disabled = busy || !state
    for (const control of [service, workspace, region, key, save]) control.disabled = busy || loadingProvider
    clear.disabled = busy || loadingProvider || !hasKey
    start.disabled = busy || !state || (!live && (!state.allowed || !player.options.length))
    form.setAttribute('aria-busy', String(busy || loadingProvider))
    start.setAttribute('aria-busy', String(busy))
  }
  const report = (message: string) => { error.textContent = message; error.hidden = !message }
  const send = async (message: Record<string, unknown>) => {
    const response = await browser.runtime.sendMessage({ ...message, tabId })
    if (!response?.ok) throw new Error(response?.error || '插件字幕服务不可用')
    return response
  }
  const renderTracks = () => {
    const known = new Set<string>((state?.players || []).flatMap((p: any) => p.tracks || []))
    const current = compactResources(pruneExpiredResources(resources), 40, true)
    for (const resource of compactResources(current, 40)) {
      for (const track of resource.audioTracks || []) {
        const label = [track.label, track.language].filter(Boolean).join(' · ')
        if (label) known.add(label)
      }
    }
    const text = known.size ? `已发现音轨：${[...known].join('、')}` : '采集当前播放器音轨；网页提供音轨信息时会提前显示。'
    if (tracks.textContent !== text) tracks.textContent = text
  }
  const onResourcesChanged: Parameters<typeof browser.storage.onChanged.addListener>[0] = (changes, area) => {
    if (disposed || area !== 'session' || !Object.hasOwn(changes, resourceKey)) return
    resourceGeneration++
    const value = changes[resourceKey].newValue
    resources = Array.isArray(value) ? value : []
  }
  const loadResources = async (pageUrl: string) => {
    const nextKey = pageResourceKey(tabId!, pageUrl)
    if (resourceKey === nextKey) return
    resourceKey = nextKey
    resources = []
    const generation = ++resourceGeneration
    try {
      const stored = await browser.storage.session.get(nextKey)
      // 初始化读取不能覆盖读取期间收到的新资源，也不能跨越页面导航。
      if (disposed || resourceKey !== nextKey || resourceGeneration !== generation) return
      resources = Array.isArray(stored[nextKey]) ? stored[nextKey] as MediaResource[] : []
    } catch {
      if (resourceKey === nextKey && resourceGeneration === generation) resourceKey = ''
    }
  }
  const refresh = async () => {
    if (disposed || root.hidden || busy || refreshing || tabId == null) return
    refreshing = true
    try {
      state = await withDeadline(send({ type: 'subtitle-state-get' }), 8_000, '字幕状态读取超时，请重试')
      if (disposed || busy) return
      const settings = state.settings
      enabled.checked = settings.enabled
      if (document.activeElement !== source) source.value = settings.source
      if (document.activeElement !== target) target.value = settings.target
      originals.checked = settings.showSource
      prefetch.checked = settings.prefetch
      if (document.activeElement !== offset) offset.value = String(settings.offset)
      const rule = (value: unknown) => value == null ? 'inherit' : value ? 'on' : 'off'
      if (document.activeElement !== site) site.value = rule(settings.sites[subtitleSite(state.pageUrl)])
      if (document.activeElement !== page) page.value = rule(state.pages[subtitlePage(state.pageUrl)])
      live = Boolean(state.session && !['stopped', 'failed'].includes(state.session.status))
      const playing = (state.players || []).some((p: any) => p.mediaId === state.session?.mediaId && p.frameId === state.session?.frameId && p.playing)
      status.textContent = !settings.enabled ? '全局已关闭' : !state.allowed ? '当前页已关闭' : live
        ? state.session.status === 'running' ? state.session.capture === 'prefetch'
          ? `同步字幕 · 已备好 ${Math.max(0, (state.session.readyEnd || 0) - (state.session.clock?.position || 0)).toFixed(1)} 秒`
          : playing ? '实时翻译中' : '等待播放器播放' : '连接中' : '已开启 · 待启动'
      start.textContent = live ? '停止翻译' : '开始翻译'
      const options = (state.players || []).map((p: any, index: number) => ({ value: `${p.frameId}:${p.mediaId}`, label: `播放器 ${index + 1}${p.playing ? ' · 播放中' : ''} · ${String(p.label).slice(0, 36)}` }))
      const old = player.value
      const signature = JSON.stringify(options)
      if (document.activeElement !== player && player.dataset.signature !== signature) {
        player.replaceChildren(...options.map((p: {value: string; label: string}) => new Option(p.label, p.value)))
        player.dataset.signature = signature
        if (options.some((p: {value: string}) => p.value === old)) player.value = old
        else {
          const active = [...(state.players || [])].sort((a, b) => Number(b.playing) - Number(a.playing) || b.area - a.area)[0]
          if (active) player.value = `${active.frameId}:${active.mediaId}`
        }
      }
      updateControls()
      await loadResources(state.pageUrl)
      renderTracks()
      if (state.session?.status === 'failed') report(state.session.error || '字幕连接失败，请重试')
    } catch (reason) {
      status.textContent = '状态读取失败'
      report(reason instanceof Error ? reason.message : String(reason))
    }
    finally { refreshing = false }
  }
  const change = async (message: Record<string, unknown>) => {
    if (busy) return
    busy = true; updateControls(); report('')
    try { await send({ type: 'subtitle-settings', ...message }) }
    catch (reason) { report(reason instanceof Error ? reason.message : String(reason)) }
    finally { busy = false; updateControls(); await refresh() }
  }
  enabled.onchange = () => void change({ settings: { enabled: enabled.checked } })
  const ruleValue = (select: HTMLSelectElement) => select.value === 'inherit' ? null : select.value === 'on'
  site.onchange = () => void change({ scope: 'site', value: ruleValue(site) })
  page.onchange = () => void change({ scope: 'page', value: ruleValue(page) })
  source.onchange = target.onchange = () => void change({ settings: { source: source.value, target: target.value } })
  offset.onchange = () => void change({ settings: { offset: offset.value } })
  originals.onchange = () => void change({ settings: { showSource: originals.checked } })
  prefetch.onchange = () => void change({ settings: { prefetch: prefetch.checked } })
  start.onclick = () => {
    if (busy) return
    busy = true; updateControls(); report('')
    const [frame, ...media] = player.value.split(':')
    void send(live ? { type: 'subtitle-stop' } : { type: 'subtitle-start', frameId: Number(frame), mediaId: media.join(':') })
      .catch(reason => report(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => { busy = false; updateControls(); void refresh() })
  }
  const loadProvider = (config: any) => {
    hasKey = Boolean(config.has_key)
    service.value = config.provider === 'qwencloud' ? 'qwencloud' : 'dashscope'
    updateProviderFields()
    workspace.value = config.workspace || ''; region.value = config.region || 'cn-beijing'
    key.value = ''; key.placeholder = config.has_key ? '已配置，留空保留原 Key' : 'API Key'
    keyState.textContent = config.has_key ? 'API Key 已加密保存' : '尚未配置 API Key'
    updateControls()
  }
  const configure = (clearKey = false) => {
    if (busy || loadingProvider) return
    busy = true; updateControls(); report('')
    const request: Record<string, unknown> = { action: 'configure', provider: service.value,
      api_key: clearKey ? '' : key.value, clear_key: clearKey }
    if (service.value === 'dashscope') { request.workspace = workspace.value.trim(); request.region = region.value }
    void send({ type: 'subtitle-provider', request })
      .then(config => { loadProvider(config); keyState.textContent = clearKey ? 'API Key 已清除' : '服务设置已保存；API Key 已由桌面端加密保存' })
      .catch(reason => report(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => { busy = false; updateControls(); void refresh() })
  }
  form.onsubmit = event => { event.preventDefault(); configure() }
  clear.onclick = () => configure(true)
  void send({ type: 'subtitle-provider', request: { action: 'configuration' } }).then(loadProvider)
    .catch(() => { keyState.textContent = '请启动新版 HLS 桌面端后配置服务' })
    .finally(() => { loadingProvider = false; updateControls() })
  updateControls()
  browser.storage.onChanged.addListener(onResourcesChanged)
  void refresh()
  const timer = setInterval(() => void refresh(), 1000)
  window.addEventListener('pagehide', () => {
    disposed = true
    clearInterval(timer)
    browser.storage.onChanged.removeListener(onResourcesChanged)
    resources = []
    key.value = ''
  }, { once: true })
}
