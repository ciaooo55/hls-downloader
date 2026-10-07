import { browser } from 'wxt/browser'
import { SUBTITLE_LANGUAGES, subtitlePage, subtitleSite } from './subtitleSettings'

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
  row.append(enabledLabel, site, page)
  const languageRow = node('div'); languageRow.className = 'subtitle-row'
  const source = node('select'); source.setAttribute('aria-label', '来源语言'); source.append(new Option('自动识别', 'auto'))
  const target = node('select'); target.setAttribute('aria-label', '目标语言')
  for (const [code, label] of SUBTITLE_LANGUAGES) { source.append(new Option(label, code)); target.append(new Option(label, code)) }
  const offset = node('input'); offset.type = 'number'; offset.step = '0.2'; offset.min = '-30'; offset.max = '30'; offset.setAttribute('aria-label', '字幕时间偏移秒')
  languageRow.append(source, node('span', '→'), target, offset, node('span', '秒'))
  const originals = node('input'); originals.type = 'checkbox'
  const originalLabel = node('label'); originalLabel.append(originals, node('span', '显示原文'))
  const prefetch = node('input'); prefetch.type = 'checkbox'; prefetch.dataset.subtitlePrefetch = 'true'
  const prefetchLabel = node('label'); prefetchLabel.append(prefetch, node('span', '提前翻译音轨（最多 30 秒）'))
  const prefetchHelp = node('small', '可独立读取的点播音轨可在暂停时预翻译，再用“下一句”快进。无法预取时使用实时采集。')
  const player = node('select'); player.setAttribute('aria-label', '字幕播放器'); player.className = 'subtitle-player'
  const start = node('button', '开始翻译'); start.className = 'hlsd-button primary'; start.dataset.subtitleStart = 'true'
  const actionRow = node('div'); actionRow.className = 'subtitle-row'; actionRow.append(player, start)
  const tracks = node('small')
  const provider = node('details'); provider.append(node('summary', '翻译服务设置（百炼）'))
  const form = node('form'); form.className = 'subtitle-provider'
  const workspace = node('input'); workspace.placeholder = '百炼业务空间 ID'; workspace.setAttribute('aria-label', '百炼业务空间 ID'); workspace.required = true
  const region = node('select'); region.setAttribute('aria-label', '翻译服务地域')
  region.append(new Option('华北 2（北京）', 'cn-beijing'), new Option('新加坡', 'ap-southeast-1'))
  const key = node('input'); key.type = 'password'; key.autocomplete = 'off'; key.placeholder = 'API Key'; key.setAttribute('aria-label', '字幕服务 API Key')
  const save = node('button', '保存服务设置'); save.type = 'submit'; save.className = 'hlsd-button'
  const clear = node('button', '清除 Key'); clear.type = 'button'; clear.className = 'hlsd-button subtle'
  const keyState = node('small', '正在读取服务配置…')
  const keyActions = node('div'); keyActions.className = 'subtitle-row'; keyActions.append(save, clear)
  form.append(workspace, region, key, keyActions, keyState, node('small', '点击开始后上传采集的网页音频；开启预翻译后会提前读取当前音轨。Key 由桌面端加密保存。'))
  provider.append(form)
  root.append(heading, row, languageRow, originalLabel, prefetchLabel, prefetchHelp, actionRow, tracks, error, provider)
  let state: any
  let busy = false
  let refreshing = false
  let live = false
  const report = (message: string) => { error.textContent = message; error.hidden = !message }
  const send = async (message: Record<string, unknown>) => {
    const response = await browser.runtime.sendMessage({ ...message, tabId })
    if (!response?.ok) throw new Error(response?.error || '插件字幕服务不可用')
    return response
  }
  const refresh = async () => {
    if (busy || refreshing || tabId == null) return
    refreshing = true
    try {
      state = await send({ type: 'subtitle-state-get' })
      if (busy) return
      const settings = state.settings
      enabled.checked = settings.enabled
      source.value = settings.source; target.value = settings.target; originals.checked = settings.showSource
      prefetch.checked = settings.prefetch
      if (document.activeElement !== offset) offset.value = String(settings.offset)
      const rule = (value: unknown) => value == null ? 'inherit' : value ? 'on' : 'off'
      site.value = rule(settings.sites[subtitleSite(state.pageUrl)])
      page.value = rule(state.pages[subtitlePage(state.pageUrl)])
      live = Boolean(state.session && !['stopped', 'failed'].includes(state.session.status))
      status.textContent = !settings.enabled ? '全局已关闭' : !state.allowed ? '当前页已关闭' : live
        ? state.session.status === 'running' ? state.session.capture === 'prefetch' ? '音轨预翻译中' : '翻译中' : '连接中' : '已开启 · 待启动'
      start.textContent = live ? '停止翻译' : '开始翻译'
      const options = (state.players || []).map((p: any, index: number) => ({ value: `${p.frameId}:${p.mediaId}`, label: `播放器 ${index + 1}${p.playing ? ' · 播放中' : ''} · ${String(p.label).slice(0, 36)}` }))
      const old = player.value
      const signature = JSON.stringify(options)
      if (player.dataset.signature !== signature) {
        player.replaceChildren(...options.map((p: {value: string; label: string}) => new Option(p.label, p.value)))
        player.dataset.signature = signature
        if (options.some((p: {value: string}) => p.value === old)) player.value = old
        else {
          const active = [...(state.players || [])].sort((a, b) => Number(b.playing) - Number(a.playing) || b.area - a.area)[0]
          if (active) player.value = `${active.frameId}:${active.mediaId}`
        }
      }
      start.disabled = !state.allowed || (!live && !options.length)
      const known = [...new Set<string>((state.players || []).flatMap((p: any) => p.tracks || []))]
      const resources = await browser.runtime.sendMessage({ type: 'list', tabId, pageUrl: state.pageUrl }).catch(() => [])
      for (const resource of Array.isArray(resources) ? resources : []) {
        for (const track of resource.audioTracks || []) {
          const label = [track.label, track.language].filter(Boolean).join(' · ')
          if (label && !known.includes(label)) known.push(label)
        }
      }
      tracks.textContent = known.length ? `已发现音轨：${known.join('、')}` : '采集当前播放器音轨；网页提供音轨信息时会提前显示。'
      if (state.session?.status === 'failed') report(state.session.error || '字幕连接失败，请重试')
    } catch (reason) { report(reason instanceof Error ? reason.message : String(reason)) }
    finally { refreshing = false }
  }
  const change = async (message: Record<string, unknown>) => {
    busy = true; start.disabled = true; report('')
    try { await send({ type: 'subtitle-settings', ...message }) }
    catch (reason) { report(reason instanceof Error ? reason.message : String(reason)) }
    finally { busy = false; await refresh() }
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
    busy = true; start.disabled = true; report('')
    const [frame, ...media] = player.value.split(':')
    void send(live ? { type: 'subtitle-stop' } : { type: 'subtitle-start', frameId: Number(frame), mediaId: media.join(':') })
      .catch(reason => report(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => { busy = false; void refresh() })
  }
  const loadProvider = (config: any) => {
    workspace.value = config.workspace || ''; region.value = config.region || 'cn-beijing'
    key.value = ''; key.placeholder = config.has_key ? '已配置，留空保留原 Key' : 'API Key'
    keyState.textContent = config.has_key ? 'API Key 已加密保存' : '尚未配置 API Key'
    clear.disabled = !config.has_key
  }
  const configure = (clearKey = false) => {
    busy = true; save.disabled = true; clear.disabled = true; report('')
    void send({ type: 'subtitle-provider', request: { action: 'configure', workspace: workspace.value.trim(), region: region.value, api_key: clearKey ? '' : key.value, clear_key: clearKey } })
      .then(loadProvider).catch(reason => report(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => { key.value = ''; busy = false; save.disabled = false; void refresh() })
  }
  form.onsubmit = event => { event.preventDefault(); configure() }
  clear.onclick = () => configure(true)
  void send({ type: 'subtitle-provider', request: { action: 'configuration' } }).then(loadProvider)
    .catch(() => { keyState.textContent = '请启动新版 HLS 桌面端后配置服务' })
  void refresh()
  const timer = setInterval(() => void refresh(), 1000)
  window.addEventListener('pagehide', () => { clearInterval(timer); key.value = '' }, { once: true })
}
