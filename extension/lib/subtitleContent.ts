import { browser } from 'wxt/browser'
import { captureSubtitleAudio } from './subtitleAudio'
import { prefetchSubtitleAudio, type PrefetchAudio } from './subtitlePrefetch'
import { subtitlePage, subtitleSettings, visibleSubtitle, type SubtitleCue, type SubtitleSettings } from './subtitleSettings'

export function installSubtitleContent(ctx: { onInvalidated(callback: () => void): void }): (node: ParentNode) => void {
  const players = new Map<HTMLMediaElement, { id: string; host: HTMLElement; button: HTMLButtonElement; caption: HTMLElement; source: HTMLElement; status: HTMLElement; back: HTMLButtonElement; next: HTMLButtonElement; earlier: HTMLButtonElement; later: HTMLButtonElement }>()
  let nextId = 0
  const documentId = [...crypto.getRandomValues(new Uint32Array(4))].map(value => value.toString(16)).join('-')
  let settings: SubtitleSettings = subtitleSettings(null)
  let allowed = false
  let selected: HTMLMediaElement | undefined
  let sessionId = ''
  let status = 'stopped'
  let error = ''
  let epoch = 0
  let stopCapture: (() => void) | undefined
  let prefetch: PrefetchAudio | undefined
  let capture = ''
  let note = ''
  const cues = new Map<string, SubtitleCue>()
  let live: { cue: SubtitleCue; at: number; position: number } | undefined
  let sourceUrl = ''
  let cueLanguages = ''
  let pageUrl = ''
  let invalidated = false
  let reporting = false
  let clockTimer: ReturnType<typeof setInterval> | undefined
  const send = (message: Record<string, unknown>) => browser.runtime.sendMessage(message)
  const button = (text: string, click: () => void) => {
    const node = document.createElement('button'); node.textContent = text; node.type = 'button'
    node.addEventListener('click', event => { event.stopPropagation(); click() }); return node
  }
  const clock = () => {
    if (selected && sessionId && !['stopped', 'failed'].includes(status)) void send({
      type: 'subtitle-clock', sessionId, position: selected.currentTime, rate: selected.playbackRate,
      playing: !selected.paused && !selected.ended && !selected.seeking && selected.readyState >= 2, epoch,
    }).catch(() => undefined)
  }
  const reset = () => { if (selected && sessionId) { epoch++; live = undefined; prefetch?.reset(epoch); clock() } }
  const pick = (media: HTMLMediaElement, id: string) => {
    if (selected) { selected.removeEventListener('seeking', reset); selected.removeEventListener('ratechange', reset) }
    stopCapture?.(); stopCapture = undefined; prefetch = undefined; capture = ''; note = ''
    const languages = `${settings.source}:${settings.target}`
    if (selected !== media || sourceUrl !== media.currentSrc || cueLanguages !== languages) cues.clear()
    cueLanguages = languages
    selected = media; sourceUrl = media.currentSrc; sessionId = id; epoch = 0; status = 'connecting'; error = ''; live = undefined
    selected.addEventListener('seeking', reset); selected.addEventListener('ratechange', reset)
    if (!clockTimer) clockTimer = setInterval(() => { clock(); render() }, 250)
    clock(); render()
  }
  const updatePolicy = async () => {
    const response = await send({ type: 'subtitle-policy-get' }).catch(() => null)
    if (!response?.ok || invalidated) return
    settings = subtitleSettings(response.settings); allowed = response.allowed === true
    if (cueLanguages && cueLanguages !== `${settings.source}:${settings.target}`) { cues.clear(); live = undefined }
    render()
  }
  const seekCue = (media: HTMLMediaElement, direction: -1 | 1) => {
    const ordered = [...cues.values()].filter(cue => cue.final && cue.translation).sort((a, b) => a.start - b.start)
    const next = direction > 0 ? ordered.find(cue => cue.start + settings.offset > media.currentTime + 0.5)
      : ordered.reverse().find(cue => cue.start + settings.offset < media.currentTime - 0.5)
    // 浏览器会截断 currentTime 精度，精确落在语句起点可能被舍入到起点之前。
    if (next) media.currentTime = Math.max(0, next.start + settings.offset + .01)
  }
  const add = (media: HTMLMediaElement) => {
    if (players.has(media)) return
    const id = `media-${documentId}-${++nextId}`
    const host = document.createElement('div'); host.dataset.hlsSubtitle = id
    const shadow = host.attachShadow({ mode: 'open' })
    const style = document.createElement('style')
    style.textContent = `:host{position:fixed;z-index:2147483646;pointer-events:none;display:block;box-sizing:border-box}
      *{box-sizing:border-box} .tools{position:absolute;left:8px;top:8px;display:flex;gap:4px;align-items:center;flex-wrap:wrap;max-width:calc(100% - 16px);pointer-events:auto;font:12px system-ui;color:white}
      button{border:1px solid #596675;border-radius:5px;padding:5px 8px;background:#17202ee8;color:#fff;cursor:pointer;font:12px system-ui}button:hover{background:#285379}button:disabled{opacity:.65;cursor:default}
      .status{padding:4px 6px;background:#111b;border-radius:4px;max-width:300px;white-space:normal}
      .captions{position:absolute;bottom:48px;left:5%;width:90%;text-align:center;font:600 clamp(15px,2vw,24px)/1.4 system-ui;color:#fff;text-shadow:0 1px 3px #000;white-space:pre-wrap;overflow-wrap:anywhere}
      .line{display:table;margin:2px auto;padding:2px 8px;background:#000b;border-radius:4px}.line:empty{display:none}.source{font-size:.76em;font-weight:400;color:#eee}`
    const tools = document.createElement('div'); tools.className = 'tools'
    const start = button('翻译字幕', () => {
      if (selected === media && !['stopped', 'failed'].includes(status)) {
        void send({ type: 'subtitle-stop' }).catch(() => undefined)
      } else {
        start.disabled = true; error = ''
        void send({ type: 'subtitle-start', mediaId: id }).then(response => {
          if (!response?.ok) { error = response?.error || '字幕启动失败'; selected = media; status = 'failed'; render() }
        }).catch(() => { error = '插件连接失败，请刷新网页'; selected = media; status = 'failed'; render() })
      }
    })
    const back = button('上一句', () => seekCue(media, -1)); back.title = '跳到已翻译的上一句'
    const next = button('下一句', () => seekCue(media, 1)); next.title = '跳到已翻译的下一句'
    const offset = (amount: number) => {
      settings.offset = Math.max(-30, Math.min(30, Math.round((settings.offset + amount) * 10) / 10))
      // 页面按钮只修改字幕时间；全局/站点/语言设置由插件弹窗管理。
      void send({ type: 'subtitle-offset', offset: settings.offset }).then(updatePolicy).catch(() => undefined)
      render()
    }
    const earlier = button('提前 0.2s', () => offset(-0.2))
    const later = button('延后 0.2s', () => offset(0.2))
    const state = document.createElement('span'); state.className = 'status'; state.setAttribute('role', 'status')
    tools.append(start, back, next, earlier, later, state)
    const captions = document.createElement('div'); captions.className = 'captions'; captions.setAttribute('aria-live', 'polite')
    const source = document.createElement('div'); source.className = 'line source'
    const caption = document.createElement('div'); caption.className = 'line translation'
    captions.append(source, caption); shadow.append(style, tools, captions)
    document.documentElement.append(host)
    players.set(media, { id, host, button: start, caption, source, status: state, back, next, earlier, later })
  }
  const inventory = async () => {
    if (invalidated || reporting) return
    reporting = true
    try {
      document.querySelectorAll<HTMLMediaElement>('video,audio').forEach(add)
      for (const [media, ui] of players) if (!media.isConnected) { ui.host.remove(); players.delete(media) }
      const values = [...players].map(([media, ui]) => {
        const rect = media.getBoundingClientRect()
        const audioTracks = (media as any).audioTracks
        const tracks = audioTracks ? Array.from(audioTracks as ArrayLike<{ label: string; language: string }>).map(t => t.label || t.language || '音轨') : []
        return { mediaId: ui.id, label: media.getAttribute('aria-label') || media.title || document.title || '网页播放器',
          playing: !media.paused && !media.ended, area: rect.width * rect.height, tracks }
      })
      const response = await send({ type: 'subtitle-players', players: values })
      if (response?.ok) {
        settings = subtitleSettings(response.settings); allowed = response.allowed === true
        const nextPage = subtitlePage(response.pageUrl)
        if (pageUrl && nextPage !== pageUrl) { cues.clear(); live = undefined }
        pageUrl = nextPage
      }
      if (selected && (!selected.isConnected || selected.currentSrc !== sourceUrl)) {
        stopCapture?.(); stopCapture = undefined
        clearInterval(clockTimer); clockTimer = undefined
        selected.removeEventListener('seeking', reset); selected.removeEventListener('ratechange', reset)
        void send({ type: 'subtitle-stop' }); selected = undefined; sessionId = ''; cues.clear(); live = undefined
      }
      render()
    } catch {} finally { reporting = false }
  }
  function render() {
    if (invalidated) return
    for (const [media, ui] of players) {
      const rect = media.getBoundingClientRect()
      const audio = media instanceof HTMLAudioElement
      const visible = rect.width >= 180 && rect.height >= (audio ? 24 : 60) && rect.bottom > 0 && rect.top < innerHeight
      ui.host.style.display = visible ? 'block' : 'none'
      const fullscreen = document.fullscreenElement
      let ancestor: Node = media
      while (ancestor.getRootNode() instanceof ShadowRoot) ancestor = (ancestor.getRootNode() as ShadowRoot).host
      const container = fullscreen && fullscreen.contains(ancestor) && fullscreen !== media ? fullscreen : document.documentElement
      if (ui.host.parentElement !== container) container.append(ui.host)
      ui.host.style.left = `${rect.left}px`; ui.host.style.top = `${rect.top}px`
      ui.host.style.width = `${rect.width}px`; ui.host.style.height = `${audio ? Math.max(rect.height, 110) : rect.height}px`
      const active = selected === media && !['stopped', 'failed'].includes(status)
      ui.back.hidden = ui.next.hidden = !allowed || !active && !(selected === media && cues.size)
      ui.earlier.hidden = ui.later.hidden = !active
      ui.status.hidden = selected !== media || (!active && !error)
      ui.button.disabled = !allowed
      ui.button.textContent = !allowed ? '字幕翻译已关闭' : active ? '停止翻译' : '翻译字幕'
      ui.button.title = !allowed ? '在 HLS 插件弹窗中开启字幕翻译，检查全局、本站和本页开关' : '在线翻译当前播放器音频'
      ui.status.textContent = selected !== media ? '' : error || (active ? status === 'running'
        ? `${capture === 'prefetch' ? '音轨预翻译中' : '翻译中'} · ${settings.offset >= 0 ? '+' : ''}${settings.offset.toFixed(1)}s${note ? ` · ${note}` : ''}` : '字幕服务连接中…' : '')
      const cue = allowed && selected === media ? visibleSubtitle(cues.values(), media.currentTime, settings.offset) : undefined
      // 同传有服务端延迟，最新译文在抵达后短暂显示；回看仍严格使用音频时间轴。
      const recent = capture !== 'prefetch' && allowed && active && live && Date.now() >= live.at + Math.max(0, settings.offset * 1000)
        && Date.now() - live.at < 4500 + settings.offset * 1000 && media.currentTime >= live.position - 0.5 ? live.cue : undefined
      const shown = cue || recent
      ui.caption.textContent = shown?.translation || ''
      ui.source.textContent = settings.showSource ? shown?.source || '' : ''
      ui.back.disabled = !allowed || selected !== media || ![...cues.values()].some(c => c.final && c.translation && c.start + settings.offset < media.currentTime - .5)
      ui.next.disabled = !allowed || selected !== media || ![...cues.values()].some(c => c.final && c.translation && c.start + settings.offset > media.currentTime + .5)
    }
  }
  const onPlay = (event: Event) => {
    const media = event.composedPath().find(node => node instanceof HTMLMediaElement) as HTMLMediaElement | undefined
    if (media) { add(media); void inventory() }
  }
  const listener = (message: any, _sender: unknown, respond: (value: unknown) => void) => {
    if (message?.type === 'subtitle-policy') { void updatePolicy(); return }
    if (message?.type === 'subtitle-inventory') { void inventory(); return }
    if (message?.type === 'subtitle-select') {
      const media = [...players].find(([, ui]) => ui.id === message.mediaId)?.[0]
      if (!media?.isConnected) { respond({ ok: false, error: '播放器已移除，请重新选择' }); return }
      pick(media, String(message.sessionId)); respond({ ok: true }); return
    }
    if (message?.type === 'subtitle-prefetch-start' && message.sessionId === sessionId && selected) {
      const controller = new AbortController()
      const id = sessionId
      stopCapture = () => controller.abort()
      void prefetchSubtitleAudio(selected, controller.signal, epoch, () => id === sessionId && status === 'running',
        (audio, position, rate, audioEpoch) => {
          if (id === sessionId) void send({ type: 'subtitle-prefetch-audio', sessionId: id, audio, position, rate,
            epoch: audioEpoch, mainPosition: selected!.currentTime, capturedAt: Date.now() }).catch(() => undefined)
        }, reason => { if (id === sessionId) void send({ type: 'subtitle-capture-error', sessionId: id, error: reason }) },
      ).then(value => {
        if (id !== sessionId || controller.signal.aborted) { value.stop(); respond({ ok: false }); return }
        prefetch = value; stopCapture = () => { controller.abort(); value.stop(); prefetch = undefined }
        respond({ ok: true })
      }).catch(reason => {
        if (id === sessionId && !controller.signal.aborted) stopCapture = undefined
        respond({ ok: false, error: reason instanceof Error ? reason.message : '无法提前读取音轨' })
      })
      return true
    }
    if (message?.type === 'subtitle-capture-start' && message.sessionId === sessionId && selected) {
      const media = selected as HTMLMediaElement & { captureStream?: () => MediaStream; mozCaptureStream?: () => MediaStream }
      const capture = media.captureStream || media.mozCaptureStream
      if (!capture) { respond({ ok: false, error: '当前浏览器不支持播放器音频采集，请使用 Edge 或 Chrome' }); return }
      const id = sessionId
      void (async () => {
        const stream = capture.call(media)
        if (!stream.getAudioTracks().length) { stream.getTracks().forEach(t => t.stop()); throw new Error('播放器未提供可采集音轨，请开始播放或使用 Edge / Chrome') }
        stream.getVideoTracks().forEach(t => t.stop())
        const cleanup = await captureSubtitleAudio(stream, false, (audio, capturedAt) => {
          if (id === sessionId) void send({ type: 'subtitle-audio', sessionId: id, audio, capturedAt }).catch(() => undefined)
        })
        if (id !== sessionId) { cleanup(); return }
        stopCapture = cleanup
        stream.getAudioTracks()[0]?.addEventListener('ended', () => { void send({ type: 'subtitle-capture-ended', sessionId: id }) })
      })().then(() => respond({ ok: true })).catch(reason => respond({ ok: false, error: reason instanceof Error ? reason.message : '播放器音频采集失败' }))
      return true
    }
    if (message?.type === 'subtitle-capture-stop' && message.sessionId === sessionId) { stopCapture?.(); stopCapture = undefined; respond({ ok: true }); return }
    if (message?.type !== 'subtitle-state' || message.sessionId !== sessionId) return
    status = message.status; error = message.error || ''; capture = message.capture || ''; note = message.note || ''
    if (['failed', 'stopped'].includes(status)) {
      stopCapture?.(); stopCapture = undefined; live = undefined
      clearInterval(clockTimer); clockTimer = undefined
    }
    for (const event of message.events || []) {
      if (event.type !== 'cue' || event.epoch !== epoch || !Number.isFinite(event.start) || !Number.isFinite(event.end)) continue
      const id = `${sessionId}:${epoch}:${String(event.id)}`
      const cue = cues.get(id) || { id, start: event.start, end: event.end, source: '', translation: '', final: false }
      cue.start = event.start; cue.end = event.end
      if (event.role === 'source') cue.source = String(event.text)
      else { cue.translation = String(event.text); cue.final = event.final === true }
      cues.set(id, cue)
      if (cue.translation && selected) live = { cue, at: Date.now(), position: selected.currentTime }
      if (cues.size > 2000) cues.delete(cues.keys().next().value!)
    }
    render()
  }
  browser.runtime.onMessage.addListener(listener)
  document.addEventListener('play', onPlay, true)
  document.addEventListener('playing', onPlay, true)
  document.addEventListener('fullscreenchange', render)
  window.addEventListener('scroll', render, { passive: true })
  window.addEventListener('resize', render)
  const inventoryTimer = setInterval(() => void inventory(), 2000)
  void inventory()
  ctx.onInvalidated(() => {
    invalidated = true; clearInterval(inventoryTimer); clearInterval(clockTimer)
    stopCapture?.(); browser.runtime.onMessage.removeListener(listener)
    selected?.removeEventListener('seeking', reset); selected?.removeEventListener('ratechange', reset)
    document.removeEventListener('play', onPlay, true); document.removeEventListener('playing', onPlay, true)
    document.removeEventListener('fullscreenchange', render); window.removeEventListener('scroll', render); window.removeEventListener('resize', render)
    for (const ui of players.values()) ui.host.remove()
  })
  // 与下载入口共用开放 Shadow DOM 的发现与动态节点观察，不再重复遍历整页。
  return node => {
    if (invalidated) return
    const count = players.size
    if (node instanceof HTMLMediaElement) add(node)
    node.querySelectorAll<HTMLMediaElement>('video,audio').forEach(add)
    if (players.size !== count) void inventory()
  }
}
