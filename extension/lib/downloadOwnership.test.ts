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
      tabs: { onUpdated: event(), onActivated: event(), onRemoved: event(), get: vi.fn().mockResolvedValue({ url: 'https://site.test/' }), query: vi.fn().mockResolvedValue([]), sendMessage: vi.fn().mockResolvedValue(undefined) },
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
  let session: Record<string, unknown>
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
    session = {}
    mocks.browser.storage.session.get.mockImplementation(async () => ({ ...session }))
    mocks.browser.storage.session.set.mockImplementation(async value => { Object.assign(session, value) })
    item = { id: 42, url: 'https://site.test/one-use', finalUrl: 'https://site.test/one-use',
      referrer: 'https://site.test/', filename: 'C:\\Downloads\\export.bin', mime: 'application/octet-stream',
      state: 'complete', paused: false, bytesReceived: 1024, totalBytes: 1024, fileSize: 1024, tabId: 7,
      startTime: '2026-10-29T00:00:00.000Z' }
    mocks.browser.downloads.search.mockImplementation(async () => item ? [{ ...item }] : [])
    mocks.browser.downloads.pause.mockImplementation(async () => {
      if (item.state === 'complete') throw new Error('already complete')
      item.paused = true
    })
    mocks.browser.downloads.removeFile.mockResolvedValue(undefined)
    mocks.browser.downloads.erase.mockImplementation(async () => { item = undefined; return [42] })
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
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
    return listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
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

  it('keeps repeated link clicks on the same uncertain offer until its receipt is recovered', async () => {
    let reachable = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'Core pipe read: closed' }
      if (message.op === 'handoff_status' && reachable) return { ok: true, handoff: { id: 'one-offer', status: 'pending' } }
      throw new Error('offline')
    })
    const first = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await first).toMatchObject({ ownership_uncertain: true, browserFallback: false })
    const second = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await second).toMatchObject({ ownership_uncertain: true, browserFallback: false })
    reachable = true
    const recovered = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await recovered).toMatchObject({ handoff: { id: 'one-offer' }, browserFallback: false })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
  })

  it('recovers an uncertain direct-link offer after the background worker reloads', async () => {
    let reachable = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'receipt lost' }
      if (message.op === 'handoff_status' && reachable) return { ok: true, handoff: { id: 'original-offer', status: 'pending' } }
      throw new Error('offline')
    })
    const first = directLink()
    await vi.advanceTimersByTimeAsync(500)
    const original = await first
    expect(original.ownership_uncertain).toBe(true)
    mocks.native.mockImplementation(async () => { throw new Error('offline') })
    vi.clearAllTimers()
    session = {}
    vi.resetModules()
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
    const offline = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await offline).toMatchObject({ ownership_uncertain: true, browserFallback: false })
    reachable = true
    mocks.native.mockImplementation(async message => message.op === 'handoff_status'
      ? { ok: true, handoff: { id: 'original-offer', status: 'pending' } } : { ok: true })
    const retry = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await retry).toMatchObject({ handoff: { id: 'original-offer' }, browserFallback: false })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'handoff_status'
      && message.client_request_id === original.client_request_id)).toBe(true)
  })

  it('reconciles an empty lookup through atomic admission with the original ID', async () => {
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'receipt lost' }
      if (message.op === 'handoff_status') return { ok: true, handoff: null }
      if (message.op === 'recover_offer') return { ok: true, handoff: { id: 'one-admission', status: 'pending' } }
      return { ok: false }
    })
    const first = directLink()
    await vi.advanceTimersByTimeAsync(500)
    const original = await first
    expect(original.ownership_uncertain).toBe(true)
    const retry = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await retry).toMatchObject({ handoff: { id: 'one-admission' }, browserFallback: false })
    const recoveries = mocks.native.mock.calls.filter(([message]) => message.op === 'recover_offer')
    expect(recoveries).toHaveLength(1)
    expect(recoveries[0][0].resource.client_request_id).toBe(original.client_request_id)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
  })

  it('recovers a persisted journal when the first worker exited before sending', async () => {
    const { localDownloadIntentUrl, resourceFingerprint } = await import('./resources')
    local['pending-direct-offers-v1'] = [{
      key: await localDownloadIntentUrl(`7:${resourceFingerprint({ url: 'https://site.test/export.zip', kind: 'file' })}`),
      tabId: 7, clientRequestId: 'download:never-sent',
    }]
    mocks.native.mockImplementation(async message => message.op === 'handoff_status'
      ? { ok: true, handoff: null }
      : { ok: true, handoff: { id: 'recovered-unsent', status: 'pending' } })
    const click = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await click).toMatchObject({ handoff: { id: 'recovered-unsent' }, browserFallback: false })
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    const recovered = mocks.native.mock.calls.find(([message]) => message.op === 'recover_offer')![0]
    expect(recovered.resource.client_request_id).toBe('download:never-sent')
  })

  it('keeps an uncertain offer parked when an older Host refuses atomic recovery', async () => {
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'handoff_status') return { ok: true, handoff: null }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'receipt lost' }
      return { ok: false, error: 'unsupported operation' }
    })
    const first = directLink()
    await vi.advanceTimersByTimeAsync(500)
    const original = await first
    const retry = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await retry).toMatchObject({ ownership_uncertain: true, browserFallback: false,
      client_request_id: original.client_request_id })
    expect((local['pending-direct-offers-v1'] as any[])[0].clientRequestId).toBe(original.client_request_id)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
  })

  it('does not treat an empty successful offer envelope as an ownership receipt', async () => {
    mocks.native.mockResolvedValue({ ok: true })
    const click = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await click).toMatchObject({ ownership_uncertain: true, browserFallback: false })
    expect(local['pending-direct-offers-v1']).toHaveLength(1)
  })

  it('journals direct-link ownership before the first native offer reply arrives', async () => {
    let release!: (value: unknown) => void
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return new Promise(resolve => { release = resolve })
      return { ok: true, handoff: { id: 'in-flight-offer', status: 'pending' } }
    })
    const first = directLink()
    await vi.advanceTimersByTimeAsync(500)
    const offered = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0]
    expect(local['pending-direct-offers-v1']).toEqual([
      { key: expect.stringMatching(/^https:\/\/local-download.invalid\/[a-f0-9]{64}$/), tabId: 7,
        clientRequestId: offered.resource.client_request_id },
    ])
    vi.clearAllTimers()
    session = {}
    vi.resetModules()
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
    const retry = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await retry).toMatchObject({ handoff: { id: 'in-flight-offer' } })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    release({ ok: true, handoff: { id: 'in-flight-offer', status: 'pending' } })
    await flush()
    await first
  })

  it('does not send an offer if its ownership journal cannot be saved', async () => {
    mocks.browser.storage.local.set.mockImplementation(async value => {
      if ('pending-direct-offers-v1' in value) throw new Error('storage unavailable')
      Object.assign(local, value)
    })
    mocks.native.mockResolvedValue({ ok: true })
    const click = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await click).toMatchObject({ ok: false, browserFallback: false })
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
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

  it('waits for a durable local import identity when storage writes fail', async () => {
    let writable = false
    mocks.browser.storage.local.set.mockImplementation(async value => {
      if ('local-browser-download-handoffs' in value && !writable) throw new Error('disk full')
      Object.assign(local, value)
    })
    response()
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    listener(mocks.browser.alarms.onAlarm)({ name: 'desktop-heartbeat' })
    await flush()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    writable = true
    await vi.advanceTimersByTimeAsync(60_000)
    const offered = mocks.native.mock.calls.find(([message]) => message.op === 'offer_local_file')?.[0]
    expect(offered).toBeDefined()
    expect(local['local-browser-download-handoffs']).toMatchObject([
      { requestId: offered.resource.client_request_id },
    ])
  })

  it.each([
    { ok: false, transport_error: true, error: 'Core pipe read: closed' },
    { ok: true },
  ])('retains local import ownership after an uncertain receipt: %j', async uncertain => {
    let reachable = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'offer_local_file') return reachable
        ? { ok: true, handoff: { id: 'local-receipt', status: 'pending' } } : uncertain
      if (message.op === 'handoff_status') return { ok: true, handoff: { id: 'local-receipt', status: 'accepted', task_status: 'completed' } }
      return { ok: false }
    })
    response(false, 'POST')
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()
    const first = mocks.native.mock.calls.find(([message]) => message.op === 'offer_local_file')![0]
    expect(local['local-browser-download-handoffs']).toMatchObject([{ requestId: first.resource.client_request_id }])
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    reachable = true
    await vi.advanceTimersByTimeAsync(30_000)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file')).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(30_000)
    const offers = mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file')
    expect(offers).toHaveLength(2)
    expect(new Set(offers.map(([message]) => message.resource.client_request_id)).size).toBe(1)
    expect(mocks.browser.downloads.removeFile).toHaveBeenCalledWith(42)
    expect(local['local-browser-download-handoffs']).toEqual([])
  })

  it('retries local import hydration after a transient storage read failure', async () => {
    mocks.browser.storage.local.get.mockImplementation(async () => ({ ...local }))
    local['local-browser-download-handoffs'] = [{ downloadId: 42, requestId: 'local:persisted',
      sourceKey: await (await import('./resources')).localDownloadIntentUrl(item.url), pageUrl: 'https://site.test/',
      importUrl: 'data:application/octet-stream,' }]
    let failed = false
    mocks.browser.storage.local.get.mockImplementation(async key => {
      if (key === 'local-browser-download-handoffs' && !failed) { failed = true; throw new Error('temporary read failure') }
      return { ...local }
    })
    vi.clearAllTimers()
    vi.resetModules()
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
    expect(failed).toBe(true)
    taskStatus = 'completed'
    listener(mocks.browser.alarms.onAlarm)({ name: 'desktop-heartbeat' })
    await flush()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file'
      && message.resource.client_request_id === 'local:persisted')).toBe(true)
    expect(mocks.browser.downloads.removeFile).toHaveBeenCalledWith(42)
  })

  it('backs off failed status envelopes without losing the admitted local import', async () => {
    let reachable = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'offer_local_file') return { ok: true, handoff: { id: 'admitted-local', status: 'pending' } }
      if (message.op === 'handoff_status') return reachable
        ? { ok: true, handoff: { id: 'admitted-local', status: 'accepted', task_status: 'completed' } }
        : { ok: false, transport_error: true, error: 'Core pipe read: closed' }
      return { ok: false }
    })
    response(false, 'POST')
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()
    const statusCalls = () => mocks.native.mock.calls.filter(([message]) => message.op === 'handoff_status').length
    expect(session['handoff-tracker-v1']).toMatchObject([{ id: 'admitted-local', status: 'connection_lost' }])
    const initialCalls = statusCalls()
    await vi.advanceTimersByTimeAsync(30_000)
    expect(statusCalls()).toBe(initialCalls)
    expect(local['local-browser-download-handoffs']).toMatchObject([{ handoffId: 'admitted-local' }])
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    reachable = true
    await vi.advanceTimersByTimeAsync(30_000)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file')).toHaveLength(1)
    expect(mocks.browser.downloads.removeFile).toHaveBeenCalledWith(42)
  })

  it.each(['chromium', 'firefox'])('preserves an unfinished one-use GET until its original bytes are complete (%s)', async family => {
    vi.stubEnv('CHROME', family === 'chromium' ? 'true' : '')
    vi.stubEnv('FIREFOX', family === 'firefox' ? 'true' : '')
    item.state = 'in_progress'
    const originalResponse = response(true)
    await vi.advanceTimersByTimeAsync(500)
    expect(await originalResponse).toBeUndefined()
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    expect(mocks.native.mock.calls.some(([message]) => ['offer', 'offer_local_file'].includes(message.op))).toBe(false)
    item.state = 'complete'
    listener(mocks.browser.downloads.onChanged)({ id: 42, state: { current: 'complete' } })
    await flush()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(true)
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
  })

  it('retains the original import identity across duplicate download events and worker restart', async () => {
    item.state = 'in_progress'
    response(true)
    const firstEvent = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await firstEvent
    const first = structuredClone(local['local-browser-download-handoffs'])
    const duplicate = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await duplicate
    expect(local['local-browser-download-handoffs']).toEqual(first)
    vi.clearAllTimers()
    vi.resetModules()
    session = {}
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
    item.state = 'complete'
    listener(mocks.browser.downloads.onChanged)({ id: 42, state: { current: 'complete' } })
    await flush()
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file').map(([message]) => message))
      .toMatchObject([{ resource: { client_request_id: (first as any[])[0].requestId } }])
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it.each(['missing-id', 'reused-id', 'ambiguous'])('restores local import identity after download IDs change: %s', async scenario => {
    item.state = 'in_progress'
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    const original = structuredClone((local['local-browser-download-handoffs'] as any[])[0])
    const impostor = { ...item, startTime: '2026-10-29T01:00:00.000Z' }
    item = { ...item, id: 7, state: 'complete' }
    mocks.browser.downloads.search.mockImplementation(async query => {
      if (query.id === 42) return scenario === 'reused-id' ? [impostor] : []
      if (query.id !== undefined) return query.id === item?.id ? [{ ...item }] : []
      return scenario === 'ambiguous' ? [{ ...item }, { ...item, id: 8 }] : [{ ...item }]
    })
    vi.clearAllTimers()
    vi.resetModules()
    session = {}
    taskStatus = 'completed'
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
    const offers = mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file')
    if (scenario === 'ambiguous') {
      expect(offers).toHaveLength(0)
      expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
      expect(local['local-browser-download-handoffs']).toMatchObject([{ requestId: original.requestId }])
    } else {
      expect(offers).toHaveLength(1)
      expect(offers[0][0].resource.client_request_id).toBe(original.requestId)
      expect(mocks.browser.downloads.removeFile).toHaveBeenCalledWith(7)
      expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalledWith(42)
    }
  })

  it('keeps a new response separate when the browser reuses an older download ID', async () => {
    response(true)
    const first = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await first
    item = { ...item, startTime: '2026-10-29T02:00:00.000Z', filename: 'C:\\Downloads\\export(1).bin' }
    response(true)
    const second = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await second
    const offers = mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file')
    expect(offers).toHaveLength(2)
    expect(new Set(offers.map(([message]) => message.resource.client_request_id)).size).toBe(2)
    expect(local['local-browser-download-handoffs']).toHaveLength(2)
  })

  it.each([false, true])('finishes the original import after browser history is lost, receipt unknown=%s', async unknownReceipt => {
    taskStatus = 'downloading'
    mocks.native.mockImplementation(async message => {
      if (message.op === 'offer_local_file') return unknownReceipt
        ? { ok: false, transport_error: true } : { ok: true, handoff: { id: 'original-import', status: 'pending' } }
      if (message.op === 'handoff_status') return { ok: true,
        handoff: { id: 'original-import', status: 'accepted', task_status: taskStatus } }
      return { ok: message.op === 'ping' }
    })
    response()
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    const original = structuredClone((local['local-browser-download-handoffs'] as any[])[0])
    mocks.browser.downloads.search.mockResolvedValue([])
    vi.clearAllTimers()
    vi.resetModules()
    session = {}
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
    expect(local['local-browser-download-handoffs']).toMatchObject([{ requestId: original.requestId }])
    taskStatus = 'completed'
    await vi.advanceTimersByTimeAsync(60_000)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file')).toHaveLength(1)
    if (unknownReceipt) expect(mocks.native.mock.calls.some(([message]) => message.op === 'handoff_status'
      && message.client_request_id === original.requestId)).toBe(true)
    expect(local['local-browser-download-handoffs']).toEqual([])
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
  })

  it('pauses an unfinished manifest before offering adaptive media', async () => {
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.state = 'in_progress'
    mocks.browser.downloads.cancel.mockImplementation(async () => { item.state = 'interrupted' })
    mocks.native.mockImplementation(async message => {
      if (message.op === 'offer') {
        expect(item.paused).toBe(true)
        return { ok: true, handoff: { id: 'handoff', status: 'pending' } }
      }
      return { ok: message.op === 'ping' }
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
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
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
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.state = 'in_progress'
    mocks.native.mockImplementation(async message => message.op === 'ping'
      ? { ok: true } : { ok: false, error: 'desktop unavailable' })
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

  it('recovers the durable receipt after offer transport retries are exhausted', async () => {
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.state = 'in_progress'
    mocks.browser.downloads.cancel.mockImplementation(async () => { item.state = 'interrupted' })
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'Core pipe read: closed' }
      if (message.op === 'handoff_status' && message.client_request_id) {
        return { ok: true, handoff: { id: 'persisted-offer', status: 'pending' } }
      }
      return { ok: false }
    })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    const offer = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0]
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'handoff_status'
      && message.client_request_id === offer.resource.client_request_id)).toBe(true)
    expect(mocks.browser.downloads.resume).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).toHaveBeenCalledWith({ id: 42 })
  })

  it('keeps unknown ownership paused and reconciles it without sending another offer', async () => {
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.state = 'in_progress'
    let reachable = false
    mocks.browser.downloads.cancel.mockImplementation(async () => { item.state = 'interrupted' })
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'Core pipe read: closed' }
      if (message.op === 'handoff_status' && message.client_request_id && reachable) {
        return { ok: true, handoff: { id: 'persisted-offer', status: 'pending' } }
      }
      throw new Error('Core disconnected')
    })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(item.paused).toBe(true)
    expect(mocks.browser.downloads.resume).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
    expect(JSON.stringify(local)).toContain('clientRequestId')
    reachable = true
    await vi.advanceTimersByTimeAsync(60_000)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    expect(mocks.browser.downloads.resume).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).toHaveBeenCalledWith({ id: 42 })
  })

  it.each(['in_progress', 'complete'])('preserves the browser %s file when a recovered handoff failed to open', async state => {
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.state = 'in_progress'
    let reachable = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'receipt lost' }
      if (message.op === 'handoff_status' && message.client_request_id && reachable) {
        return { ok: true, handoff: { id: 'failed-window', status: 'failed', presentation_ok: false } }
      }
      throw new Error('Core disconnected')
    })
    mocks.browser.downloads.resume.mockImplementation(async () => { item.paused = false })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    item.state = state
    if (state === 'complete') item.paused = false
    reachable = true
    await vi.advanceTimersByTimeAsync(60_000)
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
    if (state === 'in_progress') expect(mocks.browser.downloads.resume).toHaveBeenCalledWith(42)
    expect(local['paused-handoff-followups-v1']).toEqual([])
  })

  it('does not query a parked offer again before its retry deadline', async () => {
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.state = 'in_progress'
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'receipt lost' }
      throw new Error('Core disconnected')
    })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    const queries = () => mocks.native.mock.calls.filter(([message]) => message.op === 'handoff_status' && message.client_request_id).length
    const initialQueries = queries()
    await vi.advanceTimersByTimeAsync(30_000)
    listener(mocks.browser.alarms.onAlarm)({ name: 'desktop-heartbeat' })
    await flush()
    expect(queries()).toBe(initialQueries)
    await vi.advanceTimersByTimeAsync(30_000)
    expect(queries()).toBe(initialQueries + 1)
  })

  it.each(['in_progress', 'interrupted'])('respects a browser ownership change to %s while receipt lookup is pending', async state => {
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.state = 'in_progress'
    let recovering = false
    let release!: (value: unknown) => void
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'receipt lost' }
      if (recovering && message.op === 'handoff_status') return new Promise(resolve => { release = resolve })
      throw new Error('Core disconnected')
    })
    response(true)
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    recovering = true
    await vi.advanceTimersByTimeAsync(60_000)
    expect(release).toBeTypeOf('function')
    item.paused = false
    item.state = state
    release({ ok: true, handoff: { id: 'recovered', status: 'pending' } })
    await flush()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.resume).not.toHaveBeenCalled()
    expect(local['paused-handoff-followups-v1']).toEqual([])
  })

  it('does not cancel an original Firefox file response when the app cannot acknowledge ownership', async () => {
    vi.stubEnv('CHROME', '')
    vi.stubEnv('FIREFOX', 'true')
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true, error: 'Core pipe read: closed' }
      throw new Error('Core disconnected')
    })
    const blocked = response(true)
    await vi.advanceTimersByTimeAsync(500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
  })

  it.each(['chromium', 'firefox'])('preserves the original response when offline before any offer (%s)', async family => {
    vi.stubEnv('CHROME', family === 'chromium' ? 'true' : '')
    vi.stubEnv('FIREFOX', family === 'firefox' ? 'true' : '')
    item.state = 'in_progress'
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') throw new Error('Native Host unavailable')
      return { ok: false, transport_error: true, error: 'Native Host unavailable' }
    })
    const blocked = response(true)
    const takeover = family === 'chromium' ? listener(mocks.browser.downloads.onCreated)({ ...item }) : blocked
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    if (family === 'firefox') expect(await blocked).toBeUndefined()
    expect(item.paused).toBe(false)
    expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(local['paused-handoff-followups-v1'] || []).toEqual([])
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.erase).not.toHaveBeenCalled()
  })

  it('keeps Firefox browser fallback available after a definite application refusal', async () => {
    vi.stubEnv('CHROME', '')
    vi.stubEnv('FIREFOX', 'true')
    mocks.native.mockImplementation(async message => message.op === 'ping'
      ? { ok: true } : { ok: false, error: 'unsupported resource' })
    const blocked = response(true)
    await vi.advanceTimersByTimeAsync(500)
    expect(await blocked).toBeUndefined()
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
