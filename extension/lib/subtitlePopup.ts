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
  const site = node('select'); site.setAttribute('aria-label', '本站字幕翻译')
  const page = node('select'); page.setAttribute('aria-label', '本页字幕翻译')
  for (const [value, label] of [['inherit', '默认'], ['on', '开启'], ['off', '关闭']]) {
    site.append(new Option(`本站：${label}`, value)); page.append(new Option(`本页：${label}`, value))
  }
  row.append(page)
  const advanced = node('details'); advanced.append(node('summary', '本站与音轨'))
  const scopeRow = node('div'); scopeRow.className = 'subtitle-row'; scopeRow.append(site)
  const configuration = node('small')
  const settingsHint = node('small', '字幕服务、API Key、语言和字幕模式请在主程序“设置 → 字幕翻译”中配置。')
  const player = node('select'); player.setAttribute('aria-label', '字幕播放器'); player.className = 'subtitle-player'
  const start = node('button', '开始翻译'); start.type = 'button'; start.className = 'hlsd-button primary'; start.dataset.subtitleStart = 'true'
  const actionRow = node('div'); actionRow.className = 'subtitle-row'; actionRow.append(player, start)
  const tracks = node('small')
  advanced.append(scopeRow, tracks)
  root.append(heading, configuration, row, actionRow, error, settingsHint, advanced)
  let state: any
  let busy = false
  let refreshing = false
  let disposed = false
  let resourceKey = ''
  let resourceGeneration = 0
  let resources: MediaResource[] = []
  let live = false
  const updateControls = () => {
    for (const control of [site, page, player]) control.disabled = busy || !state
    start.disabled = busy || !state || (!live && (!state.allowed || !state.configured || !player.options.length))
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
    renderTracks()
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
      const language = (code: string) => code === 'auto' ? '自动识别' : SUBTITLE_LANGUAGES.find(([value]) => value === code)?.[1] || code
      configuration.textContent = `${language(settings.source)} → ${language(settings.target)} · ${settings.prefetch ? '同步字幕' : '实时翻译'}`
      settingsHint.textContent = `${state.configured ? '' : '尚未配置字幕服务。'}字幕服务、API Key、语言和字幕模式请在主程序“设置 → 字幕翻译”中配置。`
      const rule = (value: unknown) => value == null ? 'inherit' : value ? 'on' : 'off'
      if (document.activeElement !== site) site.value = rule(settings.sites[subtitleSite(state.pageUrl)])
      if (document.activeElement !== page) page.value = rule(state.pages[subtitlePage(state.pageUrl)])
      live = Boolean(state.session && !['stopped', 'failed'].includes(state.session.status))
      const playing = (state.players || []).some((p: any) => p.mediaId === state.session?.mediaId && p.frameId === state.session?.frameId && p.playing)
      status.textContent = !settings.enabled ? '主程序字幕已关闭' : !state.allowed ? '当前页已关闭' : live
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
      else if (live) report('')
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
  const ruleValue = (select: HTMLSelectElement) => select.value === 'inherit' ? null : select.value === 'on'
  site.onchange = () => void change({ scope: 'site', value: ruleValue(site) })
  page.onchange = () => void change({ scope: 'page', value: ruleValue(page) })
  start.onclick = () => {
    if (busy) return
    busy = true; updateControls(); report('')
    const [frame, ...media] = player.value.split(':')
    void send(live ? { type: 'subtitle-stop', sessionId: state.session.id } : { type: 'subtitle-start', frameId: Number(frame), mediaId: media.join(':') })
      .catch(reason => report(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => { busy = false; updateControls(); void refresh() })
  }
  updateControls()
  browser.storage.onChanged.addListener(onResourcesChanged)
  void refresh()
  const timer = setInterval(() => void refresh(), 1000)
  window.addEventListener('pagehide', () => {
    disposed = true
    clearInterval(timer)
    browser.storage.onChanged.removeListener(onResourcesChanged)
    resources = []
  }, { once: true })
}
