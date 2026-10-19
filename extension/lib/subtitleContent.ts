import { browser } from 'wxt/browser'
import { captureSubtitleAudio } from './subtitleAudio'
import type { MediaResource } from './resources'
import { subtitlePage, subtitleSettings, visibleSubtitle, type SubtitleCue, type SubtitleSettings } from './subtitleSettings'

export function installSubtitleContent(ctx: { onInvalidated(callback: () => void): void },
  resolveSource: (media: HTMLMediaElement) => MediaResource | undefined): (node: ParentNode) => void {
  const players = new Map<HTMLMediaElement, { id: string; host: HTMLElement; button: HTMLButtonElement; caption: HTMLElement; source: HTMLElement; status: HTMLElement; options: HTMLDetailsElement; back: HTMLButtonElement; next: HTMLButtonElement; earlier: HTMLButtonElement; later: HTMLButtonElement; hold: HTMLButtonElement }>()
  let nextId = 0
  const documentId = [...crypto.getRandomValues(new Uint32Array(4))].map(value => value.toString(16)).join('-')
  let settings: SubtitleSettings = subtitleSettings(null)
  let allowed = false
  let selected: HTMLMediaElement | undefined
  let sessionId = ''
  let status = 'stopped'
  let error = ''
  let epoch = 0
  let stopCapture: (() => void | Promise<void>) | undefined
  let readyEnd = 0
  let preparing = false
  let resumeAfterPrepare = false
  let resumePending = false
  let sourceId = ''
  let audioTrackId = ''
  const trackIdentity = (media: HTMLMediaElement) => JSON.stringify(Array.from(((media as any).audioTracks || []) as ArrayLike<{ enabled: boolean; language: string; label: string }>)
    .filter(track => track.enabled).map(track => [track.language, track.label]))
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
  const reset = () => { if (selected && sessionId) {
    epoch++; live = undefined; readyEnd = selected.currentTime
    if (capture === 'prefetch') {
      preparing = true; resumeAfterPrepare = resumeAfterPrepare || !selected.paused; selected.pause()
    }
    clock()
  } }
  const rateChanged = () => { clock(); render() }
  const pauseForBuffer = () => {
    if (!selected || selected.paused) return
    preparing = true; resumeAfterPrepare = true; selected.pause()
  }
  const pick = (media: HTMLMediaElement, id: string) => {
    if (selected) { selected.removeEventListener('seeking', reset); selected.removeEventListener('ratechange', rateChanged) }
    stopCapture?.(); stopCapture = undefined; capture = ''; note = ''; preparing = false; resumeAfterPrepare = false
    const languages = `${settings.source}:${settings.target}`
    if (selected !== media || sourceUrl !== media.currentSrc || cueLanguages !== languages) cues.clear()
    cueLanguages = languages
    selected = media; sourceUrl = media.currentSrc; sessionId = id; epoch = 0; status = 'connecting'; error = ''; live = undefined
    readyEnd = media.currentTime; sourceId = resolveSource(media)?.id || ''
    audioTrackId = trackIdentity(media)
    selected.addEventListener('seeking', reset); selected.addEventListener('ratechange', rateChanged)
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
    // 播放器顶行留给下载和更多操作；窄播放器也不能让字幕工具栏遮住它们。
    style.textContent = `:host{position:fixed;z-index:2147483646;pointer-events:none;display:block;box-sizing:border-box}
      *{box-sizing:border-box} [hidden]{display:none!important} .tools{position:absolute;right:8px;top:50px;display:flex;justify-content:flex-end;gap:4px;align-items:center;flex-wrap:wrap;max-width:calc(100% - 16px);pointer-events:auto;font:12px system-ui;color:white}
      button{border:1px solid #596675;border-radius:5px;padding:5px 8px;background:#17202ee8;color:#fff;cursor:pointer;font:12px system-ui}button:hover{background:#285379}button:disabled{opacity:.65;cursor:default}
      button:focus-visible,summary:focus-visible{outline:2px solid #9ac5ff;outline-offset:2px}
      details{position:relative}summary{padding:5px 7px;border:1px solid #596675;border-radius:5px;background:#17202ee8;cursor:pointer;list-style:none}summary::-webkit-details-marker{display:none}
      .options{position:absolute;right:0;top:32px;width:220px;max-width:calc(100vw - 24px);display:flex;flex-wrap:wrap;gap:5px;padding:8px;border:1px solid #596675;border-radius:7px;background:#17202ef5;box-shadow:0 4px 16px #0006}
      .status{padding:4px 6px;background:#111b;border-radius:4px;max-width:160px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
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
    const hold = button('保持暂停', () => { resumeAfterPrepare = false; render() })
    hold.title = '字幕准备完成后保持暂停，由你手动播放'
    const options = document.createElement('details')
    const summary = document.createElement('summary'); summary.textContent = '选项'; summary.setAttribute('aria-label', '字幕选项')
    const extra = document.createElement('div'); extra.className = 'options'
    extra.append(back, next, earlier, later, hold)
    options.append(summary, extra)
    options.addEventListener('click', event => event.stopPropagation())
    options.addEventListener('keydown', event => { if (event.key === 'Escape' && options.open) { options.open = false; summary.focus(); event.stopPropagation() } })
    const state = document.createElement('span'); state.className = 'status'; state.setAttribute('role', 'status')
    tools.append(start, options, state)
    const captions = document.createElement('div'); captions.className = 'captions'; captions.setAttribute('aria-live', 'polite')
    const source = document.createElement('div'); source.className = 'line source'
    const caption = document.createElement('div'); caption.className = 'line translation'
    captions.append(source, caption); shadow.append(style, tools, captions)
    document.documentElement.append(host)
    players.set(media, { id, host, button: start, caption, source, status: state, options, back, next, earlier, later, hold })
  }
  const inventory = async () => {
    if (invalidated || reporting) return
    reporting = true
    try {
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
        const cleanup = stopCapture; stopCapture = undefined
        try { await cleanup?.() } catch {
          void send({ type: 'subtitle-capture-error', sessionId, error: '音频尾部未能提交给字幕服务' })
        }
        clearInterval(clockTimer); clockTimer = undefined
        selected.removeEventListener('seeking', reset); selected.removeEventListener('ratechange', rateChanged)
        void send({ type: 'subtitle-stop' }); selected = undefined; sessionId = ''; cues.clear(); live = undefined
      }
      render()
    } catch {} finally { reporting = false }
  }
  function render() {
    if (invalidated) return
    if (selected && capture === 'prefetch' && status === 'running') {
      const currentSource = resolveSource(selected)
      if (currentSource && currentSource.id !== sourceId || trackIdentity(selected) !== audioTrackId || selected.mediaKeys) {
        void send({ type: 'subtitle-capture-error', sessionId, error: '播放器音源已切换，请重新开始翻译' })
        status = 'failed'; preparing = false; resumeAfterPrepare = false
      } else {
        const ahead = Math.max(0, readyEnd - selected.currentTime)
        const remaining = Math.max(0, selected.duration - selected.currentTime)
        if (!selected.paused && ahead < Math.min(1, remaining) && remaining > .05) pauseForBuffer()
        if (preparing && ahead >= Math.min(16, 8 * selected.playbackRate, remaining) - .01 && ahead > 0) {
          preparing = false
          if (resumeAfterPrepare && !resumePending && !selected.seeking) {
            resumeAfterPrepare = false; resumePending = true
            void selected.play().catch(() => { note = '字幕已准备，请点击播放器播放' }).finally(() => { resumePending = false })
          }
        }
      }
    }
    let canPrevious = false
    let canNext = false
    if (allowed && selected) for (const cue of cues.values()) {
      if (!cue.final || !cue.translation) continue
      if (cue.start + settings.offset < selected.currentTime - .5) canPrevious = true
      if (cue.start + settings.offset > selected.currentTime + .5) canNext = true
      if (canPrevious && canNext) break
    }
    for (const [media, ui] of players) {
      const rect = media.getBoundingClientRect()
      // Firefox 的原生音频文档以 40px VIDEO 呈现，紧凑控件也需要字幕显示空间。
      const visible = rect.width >= 180 && rect.height >= 24 && rect.bottom > 0 && rect.top < innerHeight
      ui.host.style.display = visible ? 'block' : 'none'
      const fullscreen = document.fullscreenElement
      let ancestor: Node = media
      while (ancestor.getRootNode() instanceof ShadowRoot) ancestor = (ancestor.getRootNode() as ShadowRoot).host
      const container = fullscreen && fullscreen.contains(ancestor) && fullscreen !== media ? fullscreen : document.documentElement
      if (ui.host.parentElement !== container) container.append(ui.host)
      ui.host.style.left = `${rect.left}px`; ui.host.style.top = `${rect.top}px`
      ui.host.style.width = `${rect.width}px`; ui.host.style.height = `${Math.max(rect.height, 110)}px`
      const active = selected === media && !['stopped', 'failed'].includes(status)
      ui.options.hidden = !allowed || !active && !(selected === media && cues.size)
      if (ui.options.hidden) ui.options.open = false
      ui.back.hidden = ui.next.hidden = !allowed || !active && !(selected === media && cues.size)
      ui.earlier.hidden = ui.later.hidden = !active
      ui.hold.hidden = !active || selected !== media || !preparing || !resumeAfterPrepare
      ui.status.hidden = selected !== media || (!active && !error)
      ui.button.disabled = !allowed
      ui.button.textContent = !allowed ? '字幕翻译已关闭' : active ? '停止翻译' : '翻译字幕'
      ui.button.title = !allowed ? '在 HLS 插件弹窗中开启字幕翻译，检查全局、本站和本页开关' : '在线翻译当前播放器音频'
      ui.status.textContent = selected !== media ? '' : error || (active ? status === 'running'
        ? `${capture === 'prefetch' ? `${preparing ? '等待字幕' : '同步字幕'} · 已备好 ${Math.max(0, readyEnd - media.currentTime).toFixed(1)} 秒` : media.paused || media.ended ? '等待播放' : '实时翻译'} · ${settings.offset >= 0 ? '+' : ''}${settings.offset.toFixed(1)}s${note ? ` · ${note}` : ''}` : '字幕服务连接中…' : '')
      ui.status.title = ui.status.textContent || ''
      const cue = allowed && selected === media ? visibleSubtitle(cues.values(), media.currentTime, settings.offset) : undefined
      // 同传有服务端延迟，最新译文在抵达后短暂显示；回看仍严格使用音频时间轴。
      const recent = capture !== 'prefetch' && allowed && active && live && Date.now() >= live.at + Math.max(0, settings.offset * 1000)
        && Date.now() - live.at < 4500 + settings.offset * 1000 && media.currentTime >= live.position - 0.5 ? live.cue : undefined
      const shown = cue || recent
      ui.caption.textContent = shown?.translation || ''
      ui.source.textContent = settings.showSource ? shown?.source || '' : ''
      ui.back.disabled = selected !== media || !canPrevious
      ui.next.disabled = selected !== media || !canNext
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
      pick(media, String(message.sessionId))
      if (message.prefetch) {
        preparing = true; resumeAfterPrepare = !media.paused; media.pause()
      }
      const tracks = Array.from(((media as any).audioTracks || []) as ArrayLike<{ enabled: boolean; label: string; language: string }>)
      const audioIndex = tracks.findIndex(track => track.enabled)
      respond({ ok: true, resourceId: sourceId, position: media.currentTime, duration: media.duration,
        drm: Boolean(media.mediaKeys), audioIndex, audioLabel: tracks[audioIndex]?.label || '', audioLanguage: tracks[audioIndex]?.language || '' })
      return
    }
    if (message?.type === 'subtitle-prefetch-start' && message.sessionId === sessionId && selected) {
      capture = 'prefetch'; preparing = true; readyEnd = selected.currentTime
      selected.pause()
      stopCapture = () => { preparing = false; resumeAfterPrepare = false }
      clock(); render(); respond({ ok: true }); return
    }
    if (message?.type === 'subtitle-capture-start' && message.sessionId === sessionId && selected) {
      const media = selected as HTMLMediaElement & { captureStream?: () => MediaStream; mozCaptureStream?: () => MediaStream }
      const capture = media.captureStream || media.mozCaptureStream
      if (!capture) { respond({ ok: false, error: '当前浏览器不支持播放器音频采集，请使用 Edge 或 Chrome' }); return }
      const id = sessionId
      void (async () => {
        const stream = capture.call(media)
        if (!stream.getAudioTracks().length) { stream.getTracks().forEach(t => t.stop()); throw new Error('播放器未提供可采集音轨，请开始播放或使用 Edge / Chrome') }
        // AudioContext 只读取音频；保持采集流完整，统一在结束时释放各轨道。
        const cleanup = await captureSubtitleAudio(stream, false, (audio, capturedAt, finalFlush) => {
          if (id === sessionId) return send({ type: 'subtitle-audio', sessionId: id, audio, capturedAt, finalFlush }).then(response => {
            if (!response?.ok) throw new Error('音频尾部未能提交给字幕服务')
          })
        })
        if (id !== sessionId) { cleanup(); return }
        stopCapture = cleanup
        stream.getAudioTracks()[0]?.addEventListener('ended', () => { void send({ type: 'subtitle-capture-ended', sessionId: id }) })
      })().then(() => respond({ ok: true })).catch(reason => respond({ ok: false, error: reason instanceof Error ? reason.message : '播放器音频采集失败' }))
      return true
    }
    if (message?.type === 'subtitle-capture-stop' && message.sessionId === sessionId) {
      const cleanup = stopCapture; stopCapture = undefined
      void Promise.resolve(cleanup?.()).then(() => respond({ ok: true }), () => respond({ ok: false, error: '音频尾部未能提交给字幕服务' }))
      return true
    }
    if (message?.type !== 'subtitle-state' || message.sessionId !== sessionId) return
    if (message.epoch !== epoch) return
    status = message.status; error = message.error || ''; capture = message.capture || ''; note = message.note || ''
    if (Number.isFinite(message.readyEnd)) readyEnd = Math.max(readyEnd, message.readyEnd)
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
  let inventoryTimer: ReturnType<typeof setTimeout> | undefined
  const scheduleInventory = () => {
    if (invalidated) return
    const active = selected && !['stopped', 'failed'].includes(status)
    inventoryTimer = setTimeout(() => { void inventory().finally(scheduleInventory) }, active || allowed && players.size > 0 ? 2000 : 10_000)
  }
  document.querySelectorAll<HTMLMediaElement>('video,audio').forEach(add)
  void inventory()
  scheduleInventory()
  ctx.onInvalidated(() => {
    invalidated = true; clearTimeout(inventoryTimer); clearInterval(clockTimer)
    stopCapture?.(); browser.runtime.onMessage.removeListener(listener)
    selected?.removeEventListener('seeking', reset); selected?.removeEventListener('ratechange', rateChanged)
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
