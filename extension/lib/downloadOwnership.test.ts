import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createHash } from 'node:crypto'

const mocks = vi.hoisted(() => {
  ;(globalThis as Record<string, unknown>).defineBackground = (main: unknown) => ({ main })
  const event = () => ({ addListener: vi.fn() })
  const storage = () => ({ get: vi.fn(), set: vi.fn(), remove: vi.fn().mockResolvedValue(undefined) })
  return {
    native: vi.fn(),
    browser: {
      runtime: { id: 'test', getManifest: () => ({ version: '7.0.3' }), getURL: () => 'chrome-extension://test/', onInstalled: event(), onMessage: event() },
      tabs: { onUpdated: event(), onActivated: event(), onRemoved: event(), get: vi.fn().mockResolvedValue({ url: 'https://site.test/' }), sendMessage: vi.fn().mockResolvedValue(undefined) },
      contextMenus: { onClicked: event(), removeAll: vi.fn().mockResolvedValue(undefined), create: vi.fn() },
      alarms: { create: vi.fn().mockResolvedValue(undefined), clear: vi.fn().mockResolvedValue(true), onAlarm: event() },
      action: { setBadgeText: vi.fn().mockResolvedValue(undefined) },
      webRequest: Object.fromEntries(['onSendHeaders', 'onBeforeRequest', 'onBeforeRedirect', 'onHeadersReceived', 'onCompleted', 'onErrorOccurred'].map(key => [key, event()])),
      downloads: { onCreated: event(), onChanged: event(), search: vi.fn(), pause: vi.fn(), resume: vi.fn(), cancel: vi.fn(), erase: vi.fn(), removeFile: vi.fn() },
      storage: { local: storage(), session: storage() },
      cookies: { getAll: vi.fn().mockResolvedValue([]) },
    },
  }
})
vi.mock('wxt/browser', () => ({ browser: mocks.browser }))
vi.mock('./nativeBridge', () => ({ NativeBridge: class { request = mocks.native } }))
vi.mock('./subtitleBackground', () => ({ installSubtitleBackground: vi.fn() }))

describe('browser download ownership through registered event listeners', () => {
  let item: any
  let local: Record<string, unknown>
  let taskStatus: string
  const listener = (event: any) => event.addListener.mock.calls.at(-1)![0]
  const flush = () => vi.advanceTimersByTimeAsync(0)

  beforeEach(async () => {
    vi.resetModules()
    vi.clearAllMocks()
    vi.useFakeTimers()
    vi.stubEnv('CHROME', 'true')
    vi.stubEnv('FIREFOX', '')
    // 使用相同 SHA-256，但同步完成摘要，避免真实线程池完成时机混入虚拟时钟的交接测试。
    vi.spyOn(crypto.subtle, 'digest').mockImplementation(async (_algorithm, data) =>
      Uint8Array.from(createHash('sha256').update(Buffer.from(data as ArrayBuffer)).digest()).buffer)
    local = { desktopClientId: 'test-client' }
    taskStatus = 'downloading'
    mocks.browser.storage.local.get.mockImplementation(async () => ({ ...local }))
    mocks.browser.storage.local.set.mockImplementation(async value => { Object.assign(local, value) })
    mocks.browser.storage.session.get.mockResolvedValue({})
    mocks.browser.storage.session.set.mockResolvedValue(undefined)
    item = { id: 42, url: 'https://site.test/one-use', finalUrl: 'https://site.test/one-use',
      referrer: 'https://site.test/', filename: 'C:\\Downloads\\export.bin', mime: 'application/octet-stream',
      state: 'complete', paused: false, bytesReceived: 1024, totalBytes: 1024, fileSize: 1024, tabId: 7 }
    mocks.browser.downloads.search.mockImplementation(async () => item ? [{ ...item }] : [])
    mocks.browser.downloads.pause.mockImplementation(async () => {
      if (item.state === 'complete') throw new Error('already complete')
      item.paused = true
    })
    mocks.browser.downloads.removeFile.mockResolvedValue(undefined)
    mocks.browser.downloads.erase.mockImplementation(async () => { item = undefined; return [42] })
    mocks.native.mockImplementation(async message => {
      if (message.op === 'offer' || message.op === 'offer_local_file') return { ok: true, handoff: { id: 'handoff', status: 'pending' } }
      if (message.op === 'handoff_status') return { ok: true, handoff: { id: 'handoff', status: 'accepted', task_status: taskStatus } }
      return { ok: false }
    })
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
  })

  afterEach(() => {
    vi.clearAllTimers()
    vi.useRealTimers()
    vi.unstubAllEnvs()
    vi.restoreAllMocks()
  })

  function response(early = false, method = 'GET') {
    const request = { requestId: 'download-request', url: item.url, method, tabId: 7,
      frameId: 0, type: 'main_frame', documentUrl: item.referrer, timeStamp: Date.now() }
    listener(mocks.browser.webRequest.onBeforeRequest)({ ...request,
      ...(method === 'POST' ? { requestBody: { formData: { export: ['once'] } } } : {}) })
    if (method === 'POST') listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
      requestHeaders: [{ name: 'Content-Type', value: 'application/x-www-form-urlencoded' }] })
    listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
      responseHeaders: [{ name: 'Content-Type', value: 'application/octet-stream' },
        ...(early ? [{ name: 'Content-Disposition', value: 'attachment; filename="export.bin"' }] : []),
        { name: 'Content-Length', value: '1024' }] })
  }

  function directLink() {
    return new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'intercept-download-link', href: 'https://site.test/export.zip', filename: '' },
      { url: 'https://site.test/', tab: { id: 7 }, frameId: 0 }, resolve,
    ))
  }

  it('shares one pending desktop confirmation for repeated direct-link clicks', async () => {
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      return { ok: true, handoff: { id: 'one-confirmation', status: 'pending' } }
    })
    const clicks = Promise.all([directLink(), directLink()])
    await vi.advanceTimersByTimeAsync(500)
    const replies = await clicks
    expect(replies.every(reply => reply.handoff?.id === 'one-confirmation')).toBe(true)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    const third = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect((await third).handoff?.id).toBe('one-confirmation')
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
  })

  it('allows browser fallback for both clicks when a shared offer is explicitly refused', async () => {
    mocks.native.mockImplementation(async message => message.op === 'ping'
      ? { ok: true } : { ok: false, error: 'desktop refused' })
    const clicks = Promise.all([directLink(), directLink()])
    await vi.advanceTimersByTimeAsync(500)
    const replies = await clicks
    expect(replies.every(reply => reply.browserFallback === true)).toBe(true)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
  })

  it('does not replay a previous POST when intercepting a new GET link to the same URL', async () => {
    item.url = item.finalUrl = 'https://site.test/export.zip'
    response(true, 'POST')
    mocks.native.mockImplementation(async message => message.op === 'ping'
      ? { ok: true } : { ok: true, handoff: { id: 'get-confirmation', status: 'pending' } })
    const click = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect((await click).browserFallback).toBe(false)
    const offered = mocks.native.mock.calls.find(([message]) => message.op === 'offer')?.[0]
    expect(offered.resource.request_method).not.toBe('POST')
    expect(offered.resource.request_body).toBeUndefined()
  })

  it.each([false, true])('imports a completed GET without replay, early response=%s', async early => {
    response(early)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item, state: 'in_progress' })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    const imported = mocks.native.mock.calls.find(([message]) => message.op === 'offer_local_file')?.[0]
    expect(imported?.resource.local_source_path).toBe('C:\\Downloads\\export.bin')
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()

    taskStatus = 'completed'
    await vi.advanceTimersByTimeAsync(1_000)
    expect(mocks.browser.downloads.removeFile).toHaveBeenCalledWith(42)
    expect(mocks.browser.downloads.erase).toHaveBeenCalledWith({ id: 42 })
  })

  it('retains the completed source when desktop import fails', async () => {
    taskStatus = 'failed'
    response()
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(true)
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
  })

  it('imports a file that completes while the pause operation is settling', async () => {
    item.state = 'in_progress'
    mocks.browser.downloads.pause.mockImplementation(async () => { item.state = 'complete' })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(true)
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
  })

  it('pauses an unfinished GET before offering its URL', async () => {
    item.state = 'in_progress'
    mocks.browser.downloads.cancel.mockImplementation(async () => { item.state = 'interrupted' })
    mocks.native.mockImplementation(async message => {
      if (message.op === 'offer') {
        expect(item.paused).toBe(true)
        return { ok: true, handoff: { id: 'handoff', status: 'pending' } }
      }
      return { ok: false }
    })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(true)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.erase).toHaveBeenCalledWith({ id: 42 })
  })

  it.each(['removed', 'canceled'])('does not offer a download %s during pause', async state => {
    item.state = 'in_progress'
    mocks.browser.downloads.pause.mockImplementation(async () => {
      item = state === 'removed' ? undefined : { ...item, state: 'interrupted', error: 'USER_CANCELED' }
    })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => ['offer', 'offer_local_file'].includes(message.op))).toBe(false)
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.resume).not.toHaveBeenCalled()
  })

  it.each(['NETWORK_FAILED', 'USER_CANCELED'])('restores a resumable %s transfer when desktop admission fails', async error => {
    item.state = 'in_progress'
    mocks.native.mockResolvedValue({ ok: false, error: 'desktop unavailable' })
    mocks.browser.downloads.pause.mockImplementation(async () => {
      item = { ...item, state: 'interrupted', paused: true, error, canResume: true }
    })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(true)
    expect(mocks.browser.downloads.resume).toHaveBeenCalledWith(42)
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
  })

  it('still imports POST responses without replaying the export request', async () => {
    response(false, 'POST')
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(true)
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
  })

  it.each([{ enabled: false }, { excludedHosts: ['site.test'] }])('preserves browser ownership for %j', async config => {
    Object.assign(local, config)
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => ['offer', 'offer_local_file'].includes(message.op))).toBe(false)
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
  })
})
