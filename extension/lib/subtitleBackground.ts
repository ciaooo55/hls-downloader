import { browser } from 'wxt/browser'
import { SUBTITLE_PAGES_KEY, SUBTITLE_SESSIONS_KEY, SUBTITLE_SETTINGS_KEY, subtitleAllowed, subtitlePage, subtitleSettings, subtitleSite } from './subtitleSettings'

interface Player { mediaId: string; label: string; playing: boolean; area: number; frameId: number; tracks: string[]; seenAt: number }
interface Clock { position: number; rate: number; playing: boolean; epoch: number; at: number }
interface Session { id: string; tabId: number; frameId: number; mediaId: string; pageUrl: string; epoch: number; sequence: number; status: string; error?: string; clock?: Clock; capture?: 'tab' | 'element' | 'prefetch'; note?: string }
const chromeAudio = () => (browser as any).tabCapture && (browser as any).offscreen

export function installSubtitleBackground(native: (request: Record<string, unknown>) => Promise<any>): void {
  let sessions: Record<string, Session> = {}
  const players = new Map<number, Map<number, Player[]>>()
  const currentPlayers = (tabId: number) => [...(players.get(tabId)?.values() || [])].flat().filter(p => Date.now() - p.seenAt < 6000)
  const ready = browser.storage.session.get(SUBTITLE_SESSIONS_KEY).then(value => { sessions = (value[SUBTITLE_SESSIONS_KEY] || {}) as Record<string, Session> })
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
    status: session.status, error: session.error || '', epoch: session.epoch, capture: session.capture, note: session.note, events,
  }, { frameId: session.frameId }).catch(() => undefined)
  const stop = async (session: Session, error = '') => {
    session.status = error ? 'failed' : 'stopped'; session.error = error
    await persist()
    await Promise.allSettled([
      call('stop', session),
      session.capture === 'tab' ? browser.runtime.sendMessage({ target: 'subtitle-offscreen', type: 'stop', sessionId: session.id })
        : browser.tabs.sendMessage(session.tabId, { type: 'subtitle-capture-stop', sessionId: session.id }, { frameId: session.frameId }),
    ])
    await notify(session)
  }
  const policy = async (pageUrl: string) => {
    const [local, temporary] = await Promise.all([browser.storage.local.get(SUBTITLE_SETTINGS_KEY), browser.storage.session.get(SUBTITLE_PAGES_KEY)])
    const settings = subtitleSettings(local[SUBTITLE_SETTINGS_KEY])
    const pages = (temporary[SUBTITLE_PAGES_KEY] || {}) as Record<string, boolean>
    return { settings, pages, allowed: subtitleAllowed(settings, pages, pageUrl) }
  }
  const ensureOffscreen = async () => {
    const offscreen = (browser as any).offscreen
    if (!await offscreen.hasDocument()) await offscreen.createDocument({
      url: browser.runtime.getURL('/subtitle-offscreen.html'), reasons: ['USER_MEDIA'],
      justification: '采集用户选择的网页音频用于在线字幕翻译，并保留原音频播放',
    })
  }
  const start = async (tabId: number, pageUrl: string, frameId?: number, mediaId?: string) => {
    const { settings, allowed } = await policy(pageUrl)
    if (!allowed) throw new Error('字幕翻译已关闭，请检查全局、本站和本页开关')
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
      await call('start', session, { source: settings.source, target: settings.target, epoch: 0 })
      const selected = await browser.tabs.sendMessage(tabId, { type: 'subtitle-select', sessionId: session.id, mediaId: session.mediaId }, { frameId: session.frameId })
      if (!selected?.ok) throw new Error(selected?.error || '播放器已离开当前页面')
      const captureElement = async () => {
        session.capture = 'element'
        const response = await browser.tabs.sendMessage(tabId, { type: 'subtitle-capture-start', sessionId: session.id, mediaId: session.mediaId }, { frameId: session.frameId })
        if (!response?.ok) throw new Error(response?.error || '当前播放器无法提供可采集音轨')
      }
      if (settings.prefetch) {
        const response = await browser.tabs.sendMessage(tabId, { type: 'subtitle-prefetch-start', sessionId: session.id }, { frameId: session.frameId })
        if (response?.ok) session.capture = 'prefetch'
        else session.note = '实时采集：' + String(response?.error || '当前音轨无法预取').slice(0, 120)
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
        if (['stopped', 'failed'].includes(session.status)) continue
        try {
          const response = await call('poll', session, { after_sequence: session.sequence })
          // 停止/换播放器期间到达的旧轮询不能复活会话。
          if (sessions[String(session.tabId)] !== session || ['stopped', 'failed'].includes(session.status)) continue
          session.status = response.status
          session.error = response.error || ''
          session.sequence = response.latest_sequence
          await notify(session, response.events || [])
          if (session.status === 'failed' || session.status === 'stopped') await stop(session, session.error)
        } catch (reason) { await stop(session, reason instanceof Error ? reason.message : String(reason)) }
      }
    } finally {
      polling = false
      if (Object.values(sessions).some(s => !['stopped', 'failed'].includes(s.status))) schedulePoll()
    }
  }
  const inFlight = new Map<string, number>()
  const audio = async (session: Session, message: any) => {
    if (session.status !== 'running' || !session.clock) return { ok: true }
    const prefetch = message.type === 'subtitle-prefetch-audio'
    if (prefetch !== (session.capture === 'prefetch') || !prefetch && !session.clock.playing) return { ok: true }
    const clock = session.clock
    const at = Number(message.capturedAt)
    if (!Number.isFinite(at) || Math.abs(Date.now() - at) > 1500 || !prefetch && Math.abs(at - clock.at) > 1500) throw new Error('字幕音频或播放器时钟已过期，请重新开始')
    if (prefetch && Number(message.epoch) !== clock.epoch) return { ok: true }
    const position = prefetch ? Number(message.position) : Math.max(0, clock.position + (at - clock.at) / 1000 * clock.rate - .25 * clock.rate)
    const rate = prefetch ? Number(message.rate) : clock.rate
    // 暂停的后台网页会节流 clock 定时器；预取块携带同时读取的主播放器位置。
    const mainPosition = prefetch ? Number(message.mainPosition) : clock.position
    if (!Number.isFinite(position) || position < 0 || !Number.isFinite(rate) || rate <= 0 || rate > 16
      || !Number.isFinite(mainPosition) || mainPosition < 0
      || prefetch && (position > mainPosition + 31 || position < mainPosition - 2)) throw new Error('预取音轨超出播放器时间窗口，请重新开始')
    const count = inFlight.get(session.id) || 0
    if (count >= 8) throw new Error('字幕发送拥堵，请检查网络后重新开始')
    inFlight.set(session.id, count + 1)
    try {
      return await call('audio', session, { audio: String(message.audio || ''), epoch: clock.epoch,
        position, rate })
    } finally { inFlight.set(session.id, (inFlight.get(session.id) || 1) - 1) }
  }
  const broadcastPolicy = async () => {
    for (const session of Object.values(sessions)) {
      if (!['stopped', 'failed'].includes(session.status) && !(await policy(session.pageUrl)).allowed) await stop(session)
    }
    const tabs = await browser.tabs.query({})
    await Promise.allSettled(tabs.filter(tab => tab.id != null).map(tab => browser.tabs.sendMessage(tab.id!, { type: 'subtitle-policy' })))
  }
  browser.runtime.onMessage.addListener((message, sender, respond) => {
    if (!message || (!String(message.type || '').startsWith('subtitle-') && message.target !== 'subtitle-background')) return
    const trustedUI = Boolean(sender.url?.startsWith(browser.runtime.getURL('/')))
    const task = async () => {
      await ready
      const tabId = Number(trustedUI ? message.tabId : sender.tab?.id)
      const tab = Number.isInteger(tabId) && tabId >= 0 ? await browser.tabs.get(tabId) : undefined
      const pageUrl = tab?.url || ''
      const session = sessions[String(tabId)]
      if (message.type === 'subtitle-players' && sender.tab) {
        const frames = players.get(tabId) || new Map<number, Player[]>()
        frames.set(sender.frameId || 0, (Array.isArray(message.players) ? message.players : []).slice(0, 40).map((p: Player) => ({ ...p, frameId: sender.frameId || 0, seenAt: Date.now() })))
        players.set(tabId, frames)
        if (session && session.frameId === (sender.frameId || 0) && !['stopped', 'failed'].includes(session.status)
          && !frames.get(session.frameId)?.some(p => p.mediaId === session.mediaId)) await stop(session)
        return { ok: true, ...(await policy(pageUrl)), pageUrl }
      }
      if (message.type === 'subtitle-policy-get' && sender.tab) return { ok: true, ...(await policy(pageUrl)), pageUrl }
      if (message.type === 'subtitle-offset' && sender.tab) return serialize(async () => {
        const { settings } = await policy(pageUrl)
        const next = subtitleSettings({ ...settings, offset: message.offset })
        await browser.storage.local.set({ [SUBTITLE_SETTINGS_KEY]: next })
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
        } else {
          const next = subtitleSettings({ ...settings, ...message.settings, sites: settings.sites })
          if (next.source !== settings.source || next.target !== settings.target || next.prefetch !== settings.prefetch) {
            for (const live of Object.values(sessions)) if (!['stopped', 'failed'].includes(live.status)) await stop(live)
          }
          await browser.storage.local.set({ [SUBTITLE_SETTINGS_KEY]: next })
        }
        await broadcastPolicy()
        return { ok: true, ...(await policy(pageUrl)) }
      })
      if (message.type === 'subtitle-provider' && trustedUI) return serialize(async () => {
        const response = await call(message.request?.action === 'configure' ? 'configure' : 'configuration', undefined, message.request || {})
        if (message.request?.action === 'configure') for (const live of Object.values(sessions)) await stop(live)
        return response
      })
      if (message.type === 'subtitle-start' && (trustedUI || sender.tab)) return serialize(() => start(tabId, pageUrl,
        sender.tab && !trustedUI ? sender.frameId || 0 : message.frameId, String(message.mediaId || '') || undefined))
      if (message.type === 'subtitle-stop' && session && (trustedUI || sender.frameId === session.frameId)) return serialize(async () => {
        await stop(session); return { ok: true }
      })
      const offscreen = sender.url === browser.runtime.getURL('/subtitle-offscreen.html')
      const live = message.sessionId ? Object.values(sessions).find(s => s.id === message.sessionId) : session
      if (!live || ['stopped', 'failed'].includes(live.status)) return { ok: false }
      if (!offscreen && (sender.tab?.id !== live.tabId || sender.frameId !== live.frameId)) return { ok: false }
      if (message.type === 'subtitle-clock') {
        const next: Clock = { position: Number(message.position), rate: Number(message.rate), playing: message.playing === true,
          epoch: Number(message.epoch), at: Date.now() }
        if (!Number.isFinite(next.position) || !Number.isFinite(next.rate) || next.rate <= 0 || !Number.isSafeInteger(next.epoch)) return { ok: false }
        live.clock = next
        if (live.epoch !== next.epoch) {
          live.epoch = next.epoch
          live.status = 'connecting'
          try { await call('reset', live, { epoch: next.epoch }) }
          catch (reason) {
            await stop(live, reason instanceof Error ? reason.message : String(reason))
            return { ok: false }
          }
        }
        return { ok: true }
      }
      if (message.type === 'audio' || message.type === 'subtitle-audio' || message.type === 'subtitle-prefetch-audio') {
        try { return await audio(live, message) }
        catch (reason) { await stop(live, reason instanceof Error ? reason.message : String(reason)); return { ok: false } }
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
    }))
  })
  browser.tabs.onUpdated.addListener((tabId, change) => {
    if (change.status === 'loading' || change.url && subtitlePage(change.url) !== subtitlePage(sessions[String(tabId)]?.pageUrl || '')) {
      players.delete(tabId)
      void ready.then(() => serialize(async () => {
        if (sessions[String(tabId)] && !['stopped', 'failed'].includes(sessions[String(tabId)].status)) await stop(sessions[String(tabId)])
      }))
    }
  })
  void ready.then(schedulePoll)
}
