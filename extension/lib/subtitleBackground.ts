import { browser } from 'wxt/browser'
import { SUBTITLE_OFFSETS_KEY, SUBTITLE_PAGES_KEY, SUBTITLE_SESSIONS_KEY, SUBTITLE_SETTINGS_KEY, subtitleAllowed, subtitleOffset, subtitlePage, subtitleReadyEnd, subtitleSettings, subtitleSite } from './subtitleSettings'
import { prefetchSubtitleAudio, type PrefetchAudio } from './subtitlePrefetch'

interface Player { mediaId: string; label: string; playing: boolean; area: number; frameId: number; tracks: string[]; seenAt: number }
interface Clock { position: number; rate: number; playing: boolean; epoch: number; at: number }
interface Session { id: string; tabId: number; frameId: number; mediaId: string; pageUrl: string; epoch: number; sequence: number; status: string; error?: string; clock?: Clock; capture?: 'tab' | 'element' | 'prefetch'; note?: string; readyEnd?: number; duration?: number; started?: boolean; readyRanges?: [number, number][] }
const chromeAudio = () => (browser as any).tabCapture && (browser as any).offscreen

export function installSubtitleBackground(native: (request: Record<string, unknown>) => Promise<any>,
  resolveSource: (tabId: number, frameId: number, selection: any) => Promise<Record<string, unknown>>): void {
  let sessions: Record<string, Session> = {}
  const prefetching = new Map<string, PrefetchAudio>()
  const players = new Map<number, Map<number, Player[]>>()
  const currentPlayers = (tabId: number) => [...(players.get(tabId)?.values() || [])].flat().filter(p => Date.now() - p.seenAt < 6000)
  const ready = browser.storage.session.get(SUBTITLE_SESSIONS_KEY)
    .then(value => { sessions = (value[SUBTITLE_SESSIONS_KEY] || {}) as Record<string, Session> })
    .catch(() => undefined)
  const persist = () => browser.storage.session.set({ [SUBTITLE_SESSIONS_KEY]: sessions })
  let operations: Promise<unknown> = Promise.resolve()
  const serialize = <T>(operation: () => Promise<T>): Promise<T> => {
    const next = operations.catch(() => undefined).then(operation)
    operations = next; return next
  }
  const call = async (action: string, session?: Session, extra: Record<string, unknown> = {}) => {
    const response = await native({ action, ...(session ? { session_id: session.id } : {}), ...extra })
    if (!response?.ok) throw new Error(String(response?.error || '字幕服务不可用'))
    return response
  }
  const notify = (session: Session, events: unknown[] = []) => browser.tabs.sendMessage(session.tabId, {
    type: 'subtitle-state', sessionId: session.id, mediaId: session.mediaId,
    status: session.status, error: session.error || '', epoch: session.epoch, capture: session.capture, note: session.note,
    readyEnd: session.readyEnd, events,
  }, { frameId: session.frameId }).catch(() => undefined)
  const stopping = new WeakMap<Session, Promise<void>>()
  const stop = (session: Session, error = '') => {
    const pending = stopping.get(session)
    if (pending) return pending
    const operation = (async () => {
      prefetching.get(session.id)?.stop(); prefetching.delete(session.id)
      session.status = 'stopping'
      session.error = error
      await persist()
      const captureStop = await Promise.allSettled([
        session.capture === 'tab' ? browser.runtime.sendMessage({ target: 'subtitle-offscreen', type: 'stop', sessionId: session.id })
          : browser.tabs.sendMessage(session.tabId, { type: 'subtitle-capture-stop', sessionId: session.id }, { frameId: session.frameId }),
      ])
      if (!error && (captureStop[0].status === 'rejected' || captureStop[0].value?.ok === false)) {
        error = '音频尾部未能提交给字幕服务'
        session.error = error
      }
      await persist()
      await Promise.allSettled([call('stop', session)])
      if (!error) {
        const deadline = Date.now() + 12000
        let terminal = false
        while (Date.now() < deadline) {
          try {
            const response = await call('poll', session, { after_sequence: session.sequence })
            session.sequence = response.latest_sequence
            if (response.events?.length) await notify(session, response.events)
            if (response.status === 'stopped') { terminal = true; break }
            if (response.status === 'failed') {
              error = response.error || '字幕结束处理失败'
              terminal = true
              break
            }
          } catch {
            error = '字幕结束状态读取失败'
            break
          }
          await new Promise(resolve => setTimeout(resolve, 100))
        }
        if (!terminal && !error) error = '字幕结束处理超时'
      }
      session.status = error ? 'failed' : 'stopped'
      session.error = error
      await persist()
      await notify(session)
    })()
    stopping.set(session, operation)
    void operation.catch(() => undefined)
    return operation
  }
  let desktopConfiguration: any
  let configurationRead: Promise<void> | undefined
  let configurationRefreshAt = 0
  const readConfiguration = (force = false): Promise<void> => {
    if (configurationRead) return configurationRead
    if (!force && Date.now() < configurationRefreshAt) return Promise.resolve()
    configurationRead = call('configuration').then(config => { desktopConfiguration = config })
      .finally(() => { configurationRefreshAt = Date.now() + 5000; configurationRead = undefined })
    return configurationRead
  }
  const policy = async (pageUrl: string, refreshConfiguration = true) => {
    const [local, temporary] = await Promise.all([
      browser.storage.local.get(SUBTITLE_SETTINGS_KEY), browser.storage.session.get([SUBTITLE_PAGES_KEY, SUBTITLE_OFFSETS_KEY]),
      refreshConfiguration ? readConfiguration().catch(() => undefined) : Promise.resolve(),
    ])
    const saved = subtitleSettings(local[SUBTITLE_SETTINGS_KEY])
    // 尚未在主程序保存过全局选项时，沿用用户已有的浏览器设置。
    const settings = subtitleSettings({ ...saved, ...desktopConfiguration?.preferences, sites: saved.sites })
    const offsets = (temporary[SUBTITLE_OFFSETS_KEY] || {}) as Record<string, number>
    const offset = offsets[subtitlePage(pageUrl)]
    if (typeof offset === 'number') settings.offset = subtitleOffset(offset)
    const pages = (temporary[SUBTITLE_PAGES_KEY] || {}) as Record<string, boolean>
    const configured = Boolean(desktopConfiguration?.has_key && (desktopConfiguration.provider === 'qwencloud' || desktopConfiguration.workspace))
    return { settings, pages, configured, allowed: subtitleAllowed(settings, pages, pageUrl) }
  }
  const ensureOffscreen = async () => {
    const offscreen = (browser as any).offscreen
    if (!await offscreen.hasDocument()) await offscreen.createDocument({
      url: browser.runtime.getURL('/subtitle-offscreen.html'), reasons: ['USER_MEDIA'],
      justification: '采集用户选择的网页音频用于在线字幕翻译，并保留原音频播放',
    })
  }
  const start = async (tabId: number, pageUrl: string, frameId?: number, mediaId?: string) => {
    await readConfiguration(true)
    const { settings, allowed } = await policy(pageUrl, false)
    if (!allowed) throw new Error('字幕翻译已关闭，请检查主程序字幕设置及当前网页开关')
    const candidates = currentPlayers(tabId)
    const chosen = candidates.filter(p => frameId == null || p.frameId === frameId).filter(p => !mediaId || p.mediaId === mediaId)
      .sort((a, b) => Number(b.playing) - Number(a.playing) || b.area - a.area)[0]
    if (!chosen) throw new Error('未找到网页播放器，请播放视频后重新打开插件')
    for (const session of Object.values(sessions)) {
      if (!['stopped', 'failed'].includes(session.status) && (chromeAudio() || session.tabId === tabId)) await stop(session)
    }
    const session: Session = { id: `hls-${crypto.randomUUID()}`, tabId, pageUrl, frameId: chosen.frameId,
      mediaId: chosen.mediaId, epoch: 0, sequence: 0, status: 'connecting' }
    sessions[String(tabId)] = session
    await persist()
    try {
      const selected = await browser.tabs.sendMessage(tabId, { type: 'subtitle-select', sessionId: session.id, mediaId: session.mediaId,
        prefetch: settings.prefetch, settings }, { frameId: session.frameId })
      if (!selected?.ok) throw new Error(selected?.error || '播放器已离开当前页面')
      let audioSource: Record<string, unknown> | undefined
      if (settings.prefetch) {
        if (selected.drm || !Number.isFinite(selected.duration) || selected.duration <= 0) {
          throw new Error('直播或受保护的音轨不能提前翻译，请在主程序关闭“同步字幕”后使用实时采集')
        }
        audioSource = await resolveSource(tabId, session.frameId, selected)
        session.duration = selected.duration
        session.readyEnd = selected.position
        session.capture = 'prefetch'
      }
      const startEpoch = session.clock?.epoch ?? 0
      session.epoch = startEpoch
      await call('start', session, { source: settings.source, target: settings.target, epoch: startEpoch,
        ...(audioSource ? { audio_source: audioSource } : {}) })
      session.started = true
      if (session.clock && session.clock.epoch !== startEpoch) {
        session.epoch = session.clock.epoch
        await call('reset', session, { epoch: session.epoch })
      }
      const startPosition = session.clock?.position ?? selected.position
      session.readyEnd = startPosition
      const captureElement = async () => {
        session.capture = 'element'
        const response = await browser.tabs.sendMessage(tabId, { type: 'subtitle-capture-start', sessionId: session.id, mediaId: session.mediaId }, { frameId: session.frameId })
        if (!response?.ok) throw new Error(response?.error || '当前播放器无法提供可采集音轨')
      }
      if (settings.prefetch) {
        const response = await browser.tabs.sendMessage(tabId, { type: 'subtitle-prefetch-start', sessionId: session.id }, { frameId: session.frameId })
        if (!response?.ok) throw new Error(response?.error || '无法准备播放器字幕缓冲')
        prefetching.set(session.id, prefetchSubtitleAudio({
          epoch: session.epoch, position: startPosition, duration: selected.duration,
          clock: () => ({ position: session.clock?.position ?? selected.position, running: session.status === 'running' }),
          prepare: (position, duration, epoch) => call('prepare_audio', session, { position, duration, epoch }),
          send: async (audio, position, epoch) => { await call('audio', session, { audio, position, rate: 1, epoch }) },
          commit: async (start, end, epoch) => { await call('commit', session, { start, end, epoch }) },
          ready: (end, epoch) => session.epoch === epoch && (session.readyEnd ?? 0) >= end - .01,
          cached: position => subtitleReadyEnd(session.readyRanges || [], position),
          failed: error => { void stop(session, error) },
        }))
      }
      if (session.capture !== 'prefetch' && chromeAudio()) {
        if (candidates.filter(p => p.playing).length > 1) throw new Error('当前页有多个播放器发声，请先暂停其他播放器')
        let streamId: string | undefined
        try { streamId = await (browser as any).tabCapture.getMediaStreamId({ targetTabId: tabId }) }
        catch (reason) {
          if (!/not been invoked|activeTab|Cannot capture/i.test(String(reason))) throw reason
          try { await captureElement() }
          catch { throw new Error('请先点击浏览器工具栏的 HLS 插件，再点击开始翻译，授予本标签页采集权限') }
        }
        if (streamId) {
          session.capture = 'tab'
          await ensureOffscreen()
          const response = await browser.runtime.sendMessage({ target: 'subtitle-offscreen', type: 'start', sessionId: session.id, streamId })
          if (!response?.ok) throw new Error(response?.error || '无法采集标签页音频')
        }
      } else if (session.capture !== 'prefetch') {
        await captureElement()
      }
      await persist()
      schedulePoll()
      return { ok: true, session }
    } catch (reason) {
      const error = String(reason instanceof Error ? reason.message : reason)
      await stop(session, /not been invoked|activeTab|Cannot capture/i.test(error) ? '请先点击浏览器工具栏的 HLS 插件，再点击开始翻译，授予本标签页采集权限' : error)
      throw new Error(session.error)
    }
  }
  let polling = false
  let timer: ReturnType<typeof setTimeout> | undefined
  function schedulePoll() {
    if (timer || polling) return
    timer = setTimeout(() => { timer = undefined; void poll() }, 600)
  }
  async function poll() {
    await ready
    if (polling) return
    polling = true
    try {
      for (const session of Object.values(sessions)) {
        if (!session.started || stopping.has(session) || ['stopped', 'failed'].includes(session.status)) continue
        try {
          const response = await call('poll', session, { after_sequence: session.sequence })
          // 停止/换播放器期间到达的旧轮询不能复活会话。
          if (sessions[String(session.tabId)] !== session || stopping.has(session) || ['stopped', 'failed'].includes(session.status)) continue
          session.status = response.status
          session.error = response.error || ''
          session.sequence = response.latest_sequence
          for (const event of response.events || []) {
            if (event.type === 'ready' && event.epoch === session.epoch && Number.isFinite(event.start)
              && Number.isFinite(event.end) && event.start <= (session.readyEnd ?? -1) + .01
              && event.end >= event.start) {
              session.readyRanges = [...(session.readyRanges || []), [event.start, event.end] as [number, number]].slice(-500)
              session.readyEnd = subtitleReadyEnd(session.readyRanges, session.clock?.position ?? event.start)
            }
          }
          await notify(session, response.events || [])
          if (session.status === 'failed' || session.status === 'stopped') await stop(session, session.error)
        } catch (reason) {
          if (!stopping.has(session)) await stop(session, reason instanceof Error ? reason.message : String(reason))
        }
      }
    } finally {
      polling = false
      if (Object.values(sessions).some(s => !stopping.has(s) && !['stopped', 'failed'].includes(s.status))) schedulePoll()
    }
  }
  const inFlight = new Map<string, number>()
  const audio = async (session: Session, message: any) => {
    const finalFlush = message.finalFlush === true
    if (session.status !== 'running' && !finalFlush || !session.clock) return { ok: true }
    if (session.capture === 'prefetch' || !session.clock.playing && !finalFlush) return { ok: true }
    const clock = session.clock
    const at = Number(message.capturedAt)
    if (!Number.isFinite(at) || !finalFlush && (Math.abs(Date.now() - at) > 1500 || Math.abs(at - clock.at) > 1500)) throw new Error('字幕音频或播放器时钟已过期，请重新开始')
    const position = finalFlush
      ? Math.max(0, clock.position - .25 * clock.rate)
      : Math.max(0, clock.position + (at - clock.at) / 1000 * clock.rate - .25 * clock.rate)
    const rate = clock.rate
    if (!Number.isFinite(position) || position < 0 || !Number.isFinite(rate) || rate <= 0 || rate > 16) throw new Error('播放器字幕时间无效，请重新开始')
    const count = inFlight.get(session.id) || 0
    if (count >= 8) throw new Error('字幕发送拥堵，请检查网络后重新开始')
    inFlight.set(session.id, count + 1)
    try {
      return await call('audio', session, { audio: String(message.audio || ''), epoch: clock.epoch,
        position, rate })
    } finally { inFlight.set(session.id, (inFlight.get(session.id) || 1) - 1) }
  }
  const broadcastPolicy = async () => {
    // 关闭开关先通知页面隐藏控件，不能等待云端尾音处理结束才更新界面。
    const tabs = await browser.tabs.query({})
    await Promise.allSettled(tabs.filter(tab => tab.id != null).map(tab => browser.tabs.sendMessage(tab.id!, { type: 'subtitle-policy' })))
    for (const session of Object.values(sessions)) {
      if (!['stopped', 'failed'].includes(session.status) && !(await policy(session.pageUrl)).allowed) await stop(session)
    }
  }
  browser.runtime.onMessage.addListener((message, sender, respond) => {
    if (!message || (!String(message.type || '').startsWith('subtitle-') && message.target !== 'subtitle-background')) return
    const trustedUI = Boolean(sender.url?.startsWith(browser.runtime.getURL('/')))
    const task = async () => {
      await ready
      const tabId = Number(trustedUI ? message.tabId : sender.tab?.id)
      const tab = Number.isInteger(tabId) && tabId >= 0 ? await browser.tabs.get(tabId).catch(() => undefined) : undefined
      const pageUrl = tab?.url || ''
      const session = sessions[String(tabId)]
      if (message.type === 'subtitle-players' && sender.tab) {
        const frames = players.get(tabId) || new Map<number, Player[]>()
        frames.set(sender.frameId || 0, (Array.isArray(message.players) ? message.players : []).slice(0, 40).map((p: Player) => ({ ...p, frameId: sender.frameId || 0, seenAt: Date.now() })))
        players.set(tabId, frames)
        if (session && session.frameId === (sender.frameId || 0) && !['stopped', 'failed'].includes(session.status)
          && !frames.get(session.frameId)?.some(p => p.mediaId === session.mediaId)) await stop(session)
        return { ok: true, ...(await policy(pageUrl, currentPlayers(tabId).length > 0)), pageUrl }
      }
      if (message.type === 'subtitle-policy-get' && sender.tab) return { ok: true, ...(await policy(pageUrl)), pageUrl }
      if (message.type === 'subtitle-offset' && sender.tab) return serialize(async () => {
        const stored = await browser.storage.session.get(SUBTITLE_OFFSETS_KEY)
        const offsets = (stored[SUBTITLE_OFFSETS_KEY] || {}) as Record<string, number>
        offsets[subtitlePage(pageUrl)] = subtitleOffset(message.offset)
        await browser.storage.session.set({ [SUBTITLE_OFFSETS_KEY]: offsets })
        await broadcastPolicy(); return { ok: true }
      })
      if (message.type === 'subtitle-state-get' && trustedUI) {
        if (tab) void browser.tabs.sendMessage(tabId, { type: 'subtitle-inventory' }).catch(() => undefined)
        schedulePoll()
        return { ok: true, ...(await policy(pageUrl)), pageUrl, session,
          players: currentPlayers(tabId) }
      }
      if (message.type === 'subtitle-settings' && trustedUI) return serialize(async () => {
        const { settings, pages } = await policy(pageUrl)
        if (message.scope === 'page' && pageUrl) {
          if (message.value == null) delete pages[subtitlePage(pageUrl)]
          else pages[subtitlePage(pageUrl)] = Boolean(message.value)
          await browser.storage.session.set({ [SUBTITLE_PAGES_KEY]: pages })
        } else if (message.scope === 'site' && pageUrl) {
          if (message.value == null) delete settings.sites[subtitleSite(pageUrl)]
          else settings.sites[subtitleSite(pageUrl)] = Boolean(message.value)
          await browser.storage.local.set({ [SUBTITLE_SETTINGS_KEY]: settings })
        } else throw new Error('请在主程序“设置 → 字幕翻译”中修改全局字幕配置')
        await broadcastPolicy()
        return { ok: true, ...(await policy(pageUrl)) }
      })
      if (message.type === 'subtitle-start' && (trustedUI || sender.tab)) return serialize(() => start(tabId, pageUrl,
        sender.tab && !trustedUI ? sender.frameId || 0 : message.frameId, String(message.mediaId || '') || undefined))
      if (message.type === 'subtitle-stop') return serialize(async () => {
        const current = sessions[String(tabId)]
        if (!current || message.sessionId !== current.id || (!trustedUI && sender.frameId !== current.frameId)) {
          return { ok: false, error: '字幕会话已结束或切换，请刷新状态后重试' }
        }
        await stop(current); return { ok: true }
      })
      const offscreen = sender.url === browser.runtime.getURL('/subtitle-offscreen.html')
      const live = message.sessionId ? Object.values(sessions).find(s => s.id === message.sessionId) : session
      if (!live || ['stopped', 'failed'].includes(live.status)) return { ok: false }
      if (!offscreen && (sender.tab?.id !== live.tabId || sender.frameId !== live.frameId)) return { ok: false }
      if (message.type === 'subtitle-clock') {
        if (stopping.has(live)) return { ok: true }
        const next: Clock = { position: Number(message.position), rate: Number(message.rate), playing: message.playing === true,
          epoch: Number(message.epoch), at: Date.now() }
        if (!Number.isFinite(next.position) || !Number.isFinite(next.rate) || next.rate <= 0 || !Number.isSafeInteger(next.epoch)) return { ok: false }
        live.clock = next
        if (!live.started) return { ok: true }
        if (live.epoch !== next.epoch) {
          live.epoch = next.epoch
          live.status = 'connecting'
          try { await call('reset', live, { epoch: next.epoch }) }
          catch (reason) {
            await stop(live, reason instanceof Error ? reason.message : String(reason))
            return { ok: false }
          }
          if (live.epoch === next.epoch) {
            live.readyEnd = subtitleReadyEnd(live.readyRanges || [], next.position)
            prefetching.get(live.id)?.reset(next.epoch, next.position)
          }
        }
        return { ok: true }
      }
      if (message.type === 'audio' || message.type === 'subtitle-audio') {
        try { return await audio(live, message) }
        catch (reason) {
          const error = reason instanceof Error ? reason.message : String(reason)
          void stop(live, error).catch(() => undefined)
          return { ok: false, error }
        }
      }
      if (message.type === 'subtitle-capture-error') { await stop(live, String(message.error || '音轨读取失败').slice(0, 160)); return { ok: true } }
      if (message.type === 'heartbeat') { await persist(); schedulePoll(); return { ok: true } }
      if (message.type === 'capture-ended' || message.type === 'subtitle-capture-ended') { await stop(live); return { ok: true } }
      return { ok: false }
    }
    void task().then(respond).catch(reason => respond({ ok: false, error: reason instanceof Error ? reason.message : String(reason) }))
    return true
  })
  browser.tabs.onRemoved.addListener(tabId => {
    players.delete(tabId)
    void ready.then(() => serialize(async () => {
      if (sessions[String(tabId)]) await stop(sessions[String(tabId)])
      delete sessions[String(tabId)]; await persist()
    })).catch(() => undefined)
  })
  browser.tabs.onUpdated.addListener((tabId, change) => {
    if (change.status === 'loading' || change.url && subtitlePage(change.url) !== subtitlePage(sessions[String(tabId)]?.pageUrl || '')) {
      players.delete(tabId)
      void ready.then(() => serialize(async () => {
        if (sessions[String(tabId)] && !['stopped', 'failed'].includes(sessions[String(tabId)].status)) await stop(sessions[String(tabId)])
      })).catch(() => undefined)
    }
  })
  void ready.then(async () => {
    for (const session of Object.values(sessions)) {
      if (session.capture === 'prefetch' && !['stopped', 'failed'].includes(session.status)) {
        await stop(session, '插件后台已重启，请重新开始提前翻译')
      }
    }
    schedulePoll()
  }).catch(() => undefined)
}
