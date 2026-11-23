import { describe, expect, it, vi } from 'vitest'
import { browser } from 'wxt/browser'
import { RequestChainStore } from './requestChain'
import { installSubtitleBackground } from './subtitleBackground'
import { SUBTITLE_SESSIONS_KEY } from './subtitleSettings'

vi.mock('wxt/browser', () => ({
  browser: {
    runtime: { getURL: (path: string) => 'moz-extension://test' + path, onMessage: { addListener: vi.fn() } },
    storage: { session: { get: vi.fn(), set: vi.fn().mockResolvedValue(undefined) } },
    tabs: { onRemoved: { addListener: vi.fn() }, onUpdated: { addListener: vi.fn() }, sendMessage: vi.fn().mockResolvedValue({ ok: true }) },
  },
}))

describe('request identity across browser events', () => {
  const initial = { requestId: 'one', url: 'https://page.test/export', tabId: 4, frameId: 0 }

  it.each(['finished', 'failed', 'tab', 'frame', 'url'])('drops cached identity after %s changes the request', change => {
    const store = new RequestChainStore()
    store.observeResponse({ ...initial, method: 'POST', statusCode: 200,
      requestHeaders: [{ name: 'Authorization', value: 'first' }],
      requestBody: { formData: { item: ['one'] } } })
    if (change === 'finished') store.finish('one')
    if (change === 'failed') store.fail('one')
    const next = { ...initial, ...(change === 'tab' && { tabId: 7 }), ...(change === 'frame' && { frameId: 2 }),
      ...(change === 'url' && { url: 'https://second.test/video' }) }
    expect(store.observeRequest(next)).toMatchObject({
      initialUrl: next.url, urls: [next.url], method: 'GET', requestBody: '', requestHeaders: {}, statusCode: 0,
    })
  })

  it('requires a unique authenticated request for exact browser downloads', () => {
    const store = new RequestChainStore()
    for (const requestId of ['first', 'second']) {
      store.observeResponse({ ...initial, requestId, statusCode: 200,
        requestHeaders: [{ name: 'Authorization', value: requestId }] })
    }
    expect(store.resolve({ url: initial.url, tabId: initial.tabId }, Date.now(), undefined, true, 'exact').kind).toBe('ambiguous')
  })

  it('does not use a third-party subrequest as the credentials for its initiating page', () => {
    const store = new RequestChainStore()
    store.observeRequest({ ...initial, url: 'https://cdn.test/audio', documentUrl: initial.url,
      type: 'xmlhttprequest', requestHeaders: [{ name: 'Authorization', value: 'cdn-only' }] })
    expect(store.pageContext(initial.tabId, initial.url)).toBeUndefined()
    expect(store.pageContext(initial.tabId, '')).toBeUndefined()
  })
})

it('keeps the latest seek alive when the superseded reset fails', async () => {
  vi.useFakeTimers()
  try {
    const session = { id: 'current', tabId: 1, frameId: 0, status: 'running', started: true, epoch: 0 }
    vi.mocked(browser.storage.session.get).mockImplementation(async () => ({ [SUBTITLE_SESSIONS_KEY]: { 1: session } }))
    let failFirst!: (error: Error) => void
    let markStarted!: () => void
    const started = new Promise<void>(resolve => { markStarted = resolve })
    const native = vi.fn(async (request: Record<string, unknown>) => {
      if (request.action === 'reset' && request.epoch === 1) {
        markStarted()
        return new Promise<never>((_, reject) => { failFirst = reject })
      }
      return { ok: true }
    })
    installSubtitleBackground(native, async () => ({}))
    const listener = vi.mocked(browser.runtime.onMessage.addListener).mock.calls[0][0]
    const seek = (epoch: number) => new Promise(resolve => listener({
      type: 'subtitle-clock', sessionId: session.id, position: epoch, rate: 1, epoch,
    }, { tab: { id: 1 }, frameId: 0 } as Parameters<typeof listener>[1], resolve))
    const first = seek(1)
    await started
    expect(await seek(2)).toEqual({ ok: true })
    failFirst(new Error('obsolete reset'))
    expect(await first).toEqual({ ok: true })
    expect(session.epoch).toBe(2)
    expect(native.mock.calls.some(([request]) => request.action === 'stop')).toBe(false)
  } finally {
    vi.clearAllTimers()
    vi.useRealTimers()
  }
})
