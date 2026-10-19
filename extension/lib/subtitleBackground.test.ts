import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { browser } from 'wxt/browser'
import { installSubtitleBackground } from './subtitleBackground'
import { SUBTITLE_SESSIONS_KEY } from './subtitleSettings'

vi.mock('wxt/browser', () => ({
  browser: {
    runtime: { getURL: (path: string) => `moz-extension://test${path}`, onMessage: { addListener: vi.fn() } },
    storage: {
      session: { get: vi.fn(), set: vi.fn().mockResolvedValue(undefined) },
      local: { get: vi.fn().mockResolvedValue({}) },
    },
    tabs: {
      get: vi.fn().mockResolvedValue({ id: 1, url: 'https://video.example/watch' }),
      sendMessage: vi.fn(), onRemoved: { addListener: vi.fn() }, onUpdated: { addListener: vi.fn() },
    },
  },
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

function install() {
  const session = { id: 'original', tabId: 1, frameId: 0, mediaId: 'player', pageUrl: 'https://video.example/watch',
    epoch: 0, sequence: 0, status: 'running', started: true, capture: 'element',
    clock: { position: 10, rate: 1, playing: true, epoch: 0, at: Date.now() } }
  vi.mocked(browser.storage.session.get).mockImplementation(async () => ({ [SUBTITLE_SESSIONS_KEY]: { '1': session } }))
  vi.mocked(browser.tabs.sendMessage).mockImplementation(async () => ({ ok: true }))
  const native = vi.fn(async (request: Record<string, unknown>): Promise<any> =>
    request.action === 'poll' ? { ok: true, status: 'stopped', latest_sequence: 0, events: [] } : { ok: true })
  installSubtitleBackground(native, async () => ({}))
  const listener = vi.mocked(browser.runtime.onMessage.addListener).mock.calls[0][0]
  const send = (message: Record<string, unknown>, popup = false): Promise<any> => new Promise(resolve => {
    const sender = popup ? { url: 'moz-extension://test/popup.html' } : { tab: { id: 1 }, frameId: 0 }
    listener(message, sender as Parameters<typeof listener>[1], resolve)
  })
  return { native, send, session }
}

describe('字幕会话的停止归属和异步响应', () => {
  beforeEach(() => { vi.clearAllMocks(); vi.useFakeTimers() })
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers() })

  it('旧页面和旧弹窗的停止请求不能结束当前会话，缺失编号也不能代停', async () => {
    const { send, native, session } = install()
    for (const popup of [false, true]) {
      for (const sessionId of ['previous', undefined]) {
        expect(await send({ type: 'subtitle-stop', tabId: 1, sessionId }, popup)).toMatchObject({ ok: false })
      }
    }
    expect(session.status).toBe('running')
    expect(native.mock.calls.some(([request]) => request.action === 'stop')).toBe(false)
    expect(await send({ type: 'subtitle-stop', sessionId: session.id })).toMatchObject({ ok: true })
    expect(session.status).toBe('stopped')
  })

  it.each(['success', 'failure'])('停止时迟到的轮询 %s 不复活会话；只提交尾音，不再重置音轨', async outcome => {
    const { send, native, session } = install()
    const poll = deferred<any>()
    const captureStopped = deferred<any>()
    const captureRequested = deferred<void>()
    native.mockImplementationOnce(() => poll.promise)
    vi.mocked(browser.tabs.sendMessage).mockImplementation(async (_tab, message) => {
      if ((message as any).type === 'subtitle-capture-stop') {
        captureRequested.resolve()
        return captureStopped.promise
      }
      return { ok: true }
    })
    await vi.advanceTimersByTimeAsync(600)
    expect(native.mock.calls[0][0]).toMatchObject({ action: 'poll', session_id: session.id })
    const stopped = send({ type: 'subtitle-stop', sessionId: session.id })
    await captureRequested.promise
    expect(session.status).toBe('stopping')
    if (outcome === 'success') poll.resolve({ ok: true, status: 'running', latest_sequence: 4, events: [] })
    else poll.reject(new Error('迟到的旧轮询失败'))
    await vi.advanceTimersByTimeAsync(0)
    expect(session.status).toBe('stopping')
    expect(browser.tabs.sendMessage).not.toHaveBeenCalledWith(1, expect.objectContaining({ status: 'running' }), expect.anything())

    await send({ type: 'subtitle-clock', sessionId: session.id, position: 20, rate: 1, playing: true, epoch: 1 })
    await send({ type: 'subtitle-audio', sessionId: session.id, audio: 'ongoing', capturedAt: Date.now() })
    await send({ type: 'subtitle-audio', sessionId: session.id, audio: 'tail', capturedAt: Date.now(), finalFlush: true })
    expect(native.mock.calls.filter(([request]) => request.action === 'reset')).toHaveLength(0)
    expect(native.mock.calls.filter(([request]) => request.action === 'audio').map(([request]) => request.audio)).toEqual(['tail'])
    expect(native.mock.calls.find(([request]) => request.action === 'audio')?.[0].epoch).toBe(0)
    captureStopped.resolve({ ok: true })
    expect(await stopped).toMatchObject({ ok: true })
    expect(session.status).toBe('stopped')
  })

  it('标签页在消息处理期间关闭时仍能完成停止与状态查询', async () => {
    const { send, session } = install()
    vi.mocked(browser.tabs.get).mockRejectedValueOnce(new Error('No tab with id: 1'))
    expect(await send({ type: 'subtitle-state-get', tabId: 1 }, true)).toMatchObject({ ok: true, pageUrl: '' })
    vi.mocked(browser.tabs.get).mockRejectedValueOnce(new Error('No tab with id: 1'))
    expect(await send({ type: 'subtitle-stop', tabId: 1, sessionId: session.id }, true)).toMatchObject({ ok: true })
    expect(session.status).toBe('stopped')
  })
})
