import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createHash } from 'node:crypto'
import { pageResourceKey } from './resources'

const mocks = vi.hoisted(() => {
  ;(globalThis as Record<string, unknown>).defineBackground = (main: unknown) => ({ main })
  const event = () => ({ addListener: vi.fn() })
  const storage = () => ({ get: vi.fn(), set: vi.fn(), remove: vi.fn().mockResolvedValue(undefined) })
  return {
    native: vi.fn(),
    inspectHls: vi.fn(),
    browser: {
      runtime: { id: 'test', getManifest: () => ({ version: '7.0.3' }), getURL: () => 'chrome-extension://test/', onInstalled: event(), onMessage: event() },
      tabs: { onUpdated: event(), onActivated: event(), onRemoved: event(), get: vi.fn().mockResolvedValue({ url: 'https://site.test/' }), query: vi.fn().mockResolvedValue([]), sendMessage: vi.fn().mockResolvedValue(undefined) },
      contextMenus: { onClicked: event(), removeAll: vi.fn().mockResolvedValue(undefined), create: vi.fn() },
      alarms: { create: vi.fn().mockResolvedValue(undefined), clear: vi.fn().mockResolvedValue(true), onAlarm: event() },
      action: { setBadgeText: vi.fn().mockResolvedValue(undefined) },
      webRequest: Object.fromEntries(['onBeforeSendHeaders', 'onSendHeaders', 'onBeforeRequest', 'onBeforeRedirect', 'onHeadersReceived', 'onCompleted', 'onErrorOccurred'].map(key => [key, event()])),
      downloads: { onCreated: event(), onChanged: event(), search: vi.fn(), pause: vi.fn(), resume: vi.fn(), cancel: vi.fn(), erase: vi.fn(), removeFile: vi.fn() },
      storage: { local: storage(), session: storage() },
      cookies: { getAll: vi.fn().mockResolvedValue([]), getAllCookieStores: vi.fn().mockResolvedValue([{ id: 'store', tabIds: [7] }]) },
    },
  }
})
vi.mock('wxt/browser', () => ({ browser: mocks.browser }))
vi.mock('./nativeBridge', () => ({ NativeBridge: class { request = mocks.native } }))
vi.mock('./subtitleBackground', () => ({ installSubtitleBackground: vi.fn() }))
vi.mock('./hlsInspection', () => ({ inspectHlsResource: mocks.inspectHls }))

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
    mocks.inspectHls.mockReset().mockResolvedValue(null)
    mocks.browser.cookies.getAll.mockReset().mockResolvedValue([])
    mocks.browser.tabs.get.mockResolvedValue({ url: 'https://site.test/' })
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

  it.each([
    { state: 'complete', paused: false },
    { state: 'interrupted', paused: false },
    { state: 'interrupted', paused: true },
  ])('keeps current download state after a stale filename event ($state, paused=$paused)', async ({ state, paused }) => {
    const filenameEvent = { addListener: vi.fn() }
    Object.assign(mocks.browser.downloads, { onDeterminingFilename: filenameEvent })
    try {
      const background = await import('../entrypoints/background')
      ;(background.default as unknown as { main(): void }).main()
      const early = { ...item, filename: 'early.bin', state: 'in_progress', paused: false, bytesReceived: 0 }
      listener(filenameEvent)(early, vi.fn())
      item = { ...item, filename: 'final.bin', state, paused, bytesReceived: 1024 }
      const refreshed = await background.refreshedDownload(item.id, { ...early, filename: '' })
      expect(refreshed).toMatchObject({ filename: 'final.bin', state, paused: item.paused, bytesReceived: 1024 })
    } finally {
      delete (mocks.browser.downloads as Record<string, unknown>).onDeterminingFilename
    }
  })

  it('uses a determined filename while the current browser snapshot still has no filename', async () => {
    const filenameEvent = { addListener: vi.fn() }
    Object.assign(mocks.browser.downloads, { onDeterminingFilename: filenameEvent })
    try {
      const background = await import('../entrypoints/background')
      ;(background.default as unknown as { main(): void }).main()
      const early = { ...item, filename: 'determined.bin', state: 'in_progress', bytesReceived: 0 }
      listener(filenameEvent)(early, vi.fn())
      item = { ...item, filename: '', bytesReceived: 1024 }
      const refreshed = await background.refreshedDownload(item.id, early)
      expect(refreshed).toMatchObject({ filename: 'determined.bin', state: 'complete', bytesReceived: 1024 })
    } finally {
      delete (mocks.browser.downloads as Record<string, unknown>).onDeterminingFilename
    }
  })

  it.each(['navigation', 'close'])('keeps other tabs resource caches during tab %s cleanup', async action => {
    const oldPage = pageResourceKey(1, 'https://site.test/old')
    const nextPage = pageResourceKey(1, 'https://site.test/next')
    const unscoped = pageResourceKey(1)
    const otherTabs = [pageResourceKey(10, 'https://site.test/old'), pageResourceKey(11), pageResourceKey(100)]
    for (const key of [oldPage, nextPage, unscoped, ...otherTabs]) session[key] = ['retained resource']
    mocks.browser.storage.session.remove.mockClear()

    if (action === 'navigation') listener(mocks.browser.tabs.onUpdated)(1, { url: 'https://site.test/next' })
    else listener(mocks.browser.tabs.onRemoved)(1)
    await flush()

    const removed = mocks.browser.storage.session.remove.mock.calls
      .flatMap(([keys]) => typeof keys === 'string' ? [keys] : keys as string[])
    expect(removed).toContain(oldPage)
    expect(removed).toContain(unscoped)
    for (const key of otherTabs) expect(removed).not.toContain(key)
    if (action === 'navigation') expect(removed).not.toContain(nextPage)
    else expect(removed).toContain(nextPage)
  })

  function resourceMessage(resource: Record<string, unknown>, frameId = 3) {
    return new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'resource', resource },
      { url: 'https://player.test/embed', tab: { id: 7 }, frameId }, resolve,
    ))
  }

  const manifestMetadata = { inspected: true, manifestType: 'media', isLive: false,
    duration: 10, variants: [], renditionUrls: [], audioTracks: [], playbackUrls: [], playbackUrlHashes: [] }

  it.each(['download-now', 'push-to-tv', 'cast-to-device'])('uses the body-associated request identity for %s', async type => {
    const url = 'https://cdn.test/get?id=42'
    for (const requestId of ['video', 'audio']) {
      const request = { requestId, url, method: 'GET', tabId: 7, frameId: 3,
        type: 'xmlhttprequest', documentUrl: 'https://player.test/embed', timeStamp: Date.now() }
      listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
        requestHeaders: [{ name: 'Authorization', value: `Bearer ${requestId}` }] })
      listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'application/octet-stream' }, { name: 'ETag', value: `"${requestId}"` }] })
    }
    await resourceMessage({ url, kind: 'hls', ...manifestMetadata, responseIdentity: { etag: '"video"' } })
    await flush()
    const stored = (session[pageResourceKey(7, 'https://site.test/')] as any[]).find(resource => resource.url === url)
    expect(stored.replayContext.request_id).toBe('video')
    const native = mocks.native.getMockImplementation()!
    mocks.native.mockImplementation(async message => message.op === 'media_push'
      ? { ok: true, id: 'push', status: 'pending' } : native(message))
    const sending = new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type, resource: stored }, { url: 'https://player.test/embed', tab: { id: 7 }, frameId: 3 }, resolve,
    ))
    await vi.advanceTimersByTimeAsync(1000)
    expect((await sending).ok).toBe(true)
    const offered = mocks.native.mock.calls.find(([message]) => message.op === (type === 'download-now' ? 'offer' : 'media_push'))![0]
    expect(offered.resource.request_headers.authorization).toBe('Bearer video')
  })

  it('does not guess credentials when a page body matches concurrent responses equally', async () => {
    const url = 'https://cdn.test/master.m3u8'
    for (const requestId of ['one', 'two']) {
      const request = { requestId, url, method: 'GET', tabId: 7, frameId: 3,
        type: 'xmlhttprequest', documentUrl: 'https://player.test/embed', timeStamp: Date.now() }
      listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
        requestHeaders: [{ name: 'Authorization', value: `Bearer ${requestId}` }] })
      listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] })
    }
    const observation = { id: 'ambiguous', url, kind: 'hls', inspected: true,
      responseIdentity: { 'content-type': 'application/vnd.apple.mpegurl' }, frameId: 3 }
    await resourceMessage(observation)
    await flush()
    const resource = (session[pageResourceKey(7, 'https://site.test/')] as any[]).find(resource => resource.url === url)
    expect(resource.replayContext.request_id).toBe('')
    expect(resource.requestHeaders).toEqual({})
    const sending = new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'download-now', resource }, { url: 'https://player.test/embed', tab: { id: 7 }, frameId: 3 }, resolve,
    ))
    await vi.advanceTimersByTimeAsync(1000)
    expect(await sending).toMatchObject({ ok: false, error: expect.stringContaining('无法唯一关联') })
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it('binds resource observations to the browser sender, not payload tab/frame identity', async () => {
    await resourceMessage({ url: 'https://cdn.test/video.mp4', kind: 'media',
      tabId: 99, frameId: 8, pageUrl: 'https://wrong.test/', evidence: ['current_src'] })
    const stored = session[pageResourceKey(7, 'https://site.test/')] as any[]
    expect(stored).toHaveLength(1)
    expect(stored[0]).toMatchObject({ tabId: 7, frameId: 3 })
    const list = (frameId: number) => new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'list' }, { url: 'https://player.test/embed', tab: { id: 7 }, frameId }, resolve,
    ))
    expect(await list(3)).toHaveLength(1)
    expect(await list(8)).toEqual([])
  })

  it('inspects a shared manifest separately for each owning frame and reuses only same-frame claims', async () => {
    mocks.inspectHls.mockResolvedValue(manifestMetadata)
    const resource = { url: 'https://cdn.test/list.m3u8', kind: 'hls' }
    await resourceMessage(resource, 3)
    await flush()
    await resourceMessage(resource, 8)
    await flush()
    await resourceMessage(resource, 8)
    await flush()
    expect(mocks.inspectHls).toHaveBeenCalledTimes(2)
    const stored = session[pageResourceKey(7, 'https://site.test/')] as any[]
    expect(stored.filter(item => item.inspected).map(item => item.frameId).sort()).toEqual([3, 8])
    expect(mocks.browser.tabs.sendMessage.mock.calls.filter(([, message]) => message.type === 'captured-resource')
      .map(([, , options]) => options.frameId).sort()).toEqual([3, 8])
  })

  it.each(['https://cdn.test/media/child.m3u8?track=video', 'https://video.test/child.m3u8'])('uses the observed target identity when inspecting rendition %s', async childUrl => {
    const masterUrl = 'https://cdn.test/media/master.m3u8'
    for (const [requestId, url, frameId] of [
      ['master', masterUrl, 3], ['child', childUrl, 3], ['other-frame', childUrl, 8],
    ] as const) {
      const request = { requestId, url, method: 'GET', tabId: 7, frameId,
        type: 'xmlhttprequest', documentUrl: 'https://player.test/embed', timeStamp: Date.now() }
      listener(mocks.browser.webRequest.onSendHeaders)({ ...request, requestHeaders: [
        { name: 'Authorization', value: `Bearer ${requestId}` },
        { name: 'X-Playback-Token', value: requestId },
        { name: 'Referer', value: 'https://player.test/' },
      ] })
      listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] })
    }
    await flush()
    const probe = mocks.inspectHls.mock.calls.find(([resource]) => resource.url === masterUrl)![0]
    const masterContext = await probe.requestContextForUrl(masterUrl)
    const childContext = await probe.requestContextForUrl(childUrl)
    expect(masterContext.requestHeaders.authorization).toBe('Bearer master')
    expect(childContext.requestHeaders.authorization).toBe('Bearer child')
    expect(childContext.requestHeaders['x-playback-token']).toBe('child')
    expect(childContext.inspectionHeaders).toEqual({ referer: 'https://player.test/' })
    const unknownContext = await probe.requestContextForUrl('https://unknown.test/child.m3u8')
    expect(unknownContext.requestHeaders).toBeUndefined()
  })

  it('does not inspect extension requests, POST manifests or responses without a media body', async () => {
    for (const [index, [tabId, method, statusCode]] of [
      [-1, 'GET', 200], [7, 'POST', 200], [7, 'GET', 204], [7, 'GET', 205], [7, 'GET', 302],
    ].entries()) {
      listener(mocks.browser.webRequest.onHeadersReceived)({ requestId: `skip-${index}`,
        url: `https://cdn.test/skip-${index}.m3u8`, tabId, frameId: 0, method, statusCode,
        type: 'xmlhttprequest', documentUrl: 'https://site.test/', timeStamp: Date.now(),
        responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] })
    }
    await flush()
    expect(mocks.inspectHls).not.toHaveBeenCalled()
  })

  it.each([204, 205, 301, 302, 307, 308, 401, 403, 404, 503])('does not publish a media candidate from HTTP %s', async statusCode => {
    const url = `https://cdn.test/rejected-${statusCode}.m3u8`
    mocks.browser.tabs.sendMessage.mockClear()
    listener(mocks.browser.webRequest.onHeadersReceived)({ requestId: `rejected-${statusCode}`,
      url, tabId: 7, frameId: 3, method: 'GET', statusCode,
      type: 'xmlhttprequest', documentUrl: 'https://site.test/', timeStamp: Date.now(),
      responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] })
    await flush()
    expect(Object.values(session).flat().some((resource: any) => resource?.url === url)).toBe(false)
    expect(mocks.browser.tabs.sendMessage.mock.calls.some(([, message]) => message.resource?.url === url)).toBe(false)
    expect(mocks.inspectHls).not.toHaveBeenCalled()
  })

  it('retains a cached media response and its credentials without probing an empty 304 body', async () => {
    const request = { requestId: 'cached-manifest', url: 'https://cdn.test/cached.m3u8?sig=one',
      tabId: 7, frameId: 3, method: 'GET', type: 'xmlhttprequest',
      documentUrl: 'https://site.test/', timeStamp: Date.now() }
    listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
      requestHeaders: [{ name: 'Authorization', value: 'Bearer cached' }] })
    listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 304,
      responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] })
    await flush()
    expect(Object.values(session).flat().find((resource: any) => resource?.url === request.url))
      .toMatchObject({ statusCode: 304, requestHeaders: { authorization: 'Bearer cached' } })
    expect(mocks.inspectHls).not.toHaveBeenCalled()
  })

  it.each(['m3u8', 'mpd'])('does not turn login pages or JSON errors at a .%s URL into media', async extension => {
    const url = `https://cdn.test/protected.${extension}`
    mocks.browser.tabs.sendMessage.mockClear()
    for (const mime of ['text/html; charset=utf-8', 'application/xhtml+xml', 'application/json', 'image/svg+xml']) {
      listener(mocks.browser.webRequest.onHeadersReceived)({ requestId: `login-${mime}`,
        url, tabId: 7, frameId: 3, method: 'GET', statusCode: 200,
        type: 'xmlhttprequest', documentUrl: 'https://site.test/', timeStamp: Date.now(),
        responseHeaders: [{ name: 'Content-Type', value: mime }] })
    }
    await flush()
    expect(Object.values(session).flat().some((resource: any) => resource?.url === url)).toBe(false)
    expect(mocks.browser.tabs.sendMessage.mock.calls.some(([, message]) => message.resource?.url === url)).toBe(false)
    expect(mocks.inspectHls).not.toHaveBeenCalled()
    // XML is a normal DASH server MIME and must remain eligible.
    listener(mocks.browser.webRequest.onHeadersReceived)({ requestId: 'xml-manifest',
      url: 'https://cdn.test/valid.mpd', tabId: 7, frameId: 3, method: 'GET', statusCode: 200,
      type: 'xmlhttprequest', documentUrl: 'https://site.test/', timeStamp: Date.now(),
      responseHeaders: [{ name: 'Content-Type', value: 'application/xml' }] })
    await flush()
    expect(Object.values(session).flat().find((resource: any) => resource?.url === 'https://cdn.test/valid.mpd'))
      .toMatchObject({ kind: 'dash' })
  })

  it.each([true, false])('stops only a sufficient page manifest, complete=%s', async complete => {
    const request = { requestId: 'page-body', url: 'https://cdn.test/body.m3u8', tabId: 7,
      frameId: 3, method: 'GET', type: 'xmlhttprequest', documentUrl: 'https://player.test/embed', timeStamp: Date.now() }
    let release!: (value: unknown) => void
    mocks.inspectHls.mockImplementation(() => new Promise(resolve => { release = resolve }))
    listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
      requestHeaders: [{ name: 'Authorization', value: 'Bearer page' }] })
    listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
      responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }, { name: 'ETag', value: '"body"' }] })
    await flush()
    const probe = mocks.inspectHls.mock.calls[0][0]
    await resourceMessage({ url: request.url, kind: 'hls', inspected: true,
      manifestType: complete ? 'media' : 'master',
      responseIdentity: { 'content-type': 'application/vnd.apple.mpegurl', etag: '"body"' },
    })
    await flush()
    expect(probe.signal.aborted).toBe(complete)
    release(manifestMetadata)
    await flush()
  })

  it('keeps in-flight inspection and exact credentials across fragment navigation', async () => {
    const url = 'https://cdn.test/list.m3u8?token=one'
    const request = { requestId: 'fragment-media', url, method: 'GET', tabId: 7, frameId: 3,
      type: 'xmlhttprequest', documentUrl: 'https://player.test/embed', timeStamp: Date.now() }
    let release!: (value: unknown) => void
    mocks.inspectHls.mockImplementation(() => new Promise(resolve => { release = resolve }))
    listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
      requestHeaders: [{ name: 'Authorization', value: 'Bearer fragment-media' }] })
    listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
      responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] })
    await flush()
    const probe = mocks.inspectHls.mock.calls[0][0]
    mocks.browser.tabs.get.mockResolvedValue({ url: 'https://site.test/#chapter-2' })
    listener(mocks.browser.tabs.onUpdated)(7, { url: 'https://site.test/#chapter-2' })
    await flush()
    expect(probe.signal.aborted).toBe(false)
    expect((await probe.requestContextForUrl(url)).requestHeaders.authorization).toBe('Bearer fragment-media')
    release(manifestMetadata)
    await flush()
    const stored = (session[pageResourceKey(7, 'https://site.test/')] as any[]).find(item => item.inspected)
    expect(stored.replayContext.request_id).toBe('fragment-media')
    const sending = new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'download-now', resource: stored },
      { url: 'https://player.test/embed', tab: { id: 7 }, frameId: 3 }, resolve,
    ))
    await vi.advanceTimersByTimeAsync(1000)
    expect((await sending).ok).toBe(true)
    expect(mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0]
      .resource.request_headers.authorization).toBe('Bearer fragment-media')
  })

  it.each(['navigation', 'query', 'reload', 'close'])('discards manifest inspection after source tab %s', async action => {
    let release!: (value: unknown) => void
    mocks.inspectHls.mockImplementation(() => new Promise(resolve => { release = resolve }))
    await resourceMessage({ url: 'https://cdn.test/list.m3u8', kind: 'hls' })
    await flush()
    expect(release).toBeTypeOf('function')
    const signal = mocks.inspectHls.mock.calls[0][0].signal as AbortSignal
    expect(signal?.aborted).toBe(false)
    if (action === 'close') listener(mocks.browser.tabs.onRemoved)(7)
    else {
      const url = action === 'query' ? 'https://site.test/?episode=2' : 'https://site.test/next'
      if (action !== 'reload') mocks.browser.tabs.get.mockResolvedValue({ url })
      listener(mocks.browser.tabs.onUpdated)(7, action === 'reload' ? { status: 'loading', url: 'https://site.test/' } : { url })
    }
    await flush()
    expect(signal.aborted).toBe(true)
    release(manifestMetadata)
    await flush()
    expect(session[pageResourceKey(7, 'https://site.test/next')]).toBeUndefined()
    expect((session[pageResourceKey(7, 'https://site.test/')] as any[] || []).some(item => item.inspected)).toBe(false)
    expect(mocks.browser.tabs.sendMessage.mock.calls.filter(([, message]) => message.type === 'captured-resource')).toEqual([])
  })

  it('does not relabel a manifest if navigation races the final save', async () => {
    let release!: (value: unknown) => void
    mocks.inspectHls.mockImplementation(() => new Promise(resolve => { release = resolve }))
    await resourceMessage({ url: 'https://cdn.test/list.m3u8', kind: 'hls' })
    await flush()
    mocks.browser.tabs.get.mockResolvedValueOnce({ url: 'https://site.test/' })
      .mockResolvedValue({ url: 'https://site.test/next' })
    release(manifestMetadata)
    await flush()
    expect(session[pageResourceKey(7, 'https://site.test/next')]).toBeUndefined()
    expect(mocks.browser.tabs.sendMessage.mock.calls.filter(([, message]) => message.type === 'captured-resource')).toEqual([])
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

  function directLink(href = 'https://site.test/export.zip') {
    return new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'intercept-download-link', href, filename: '' },
      { url: 'https://site.test/', tab: { id: 7 }, frameId: 0 }, resolve,
    ))
  }

  async function firefoxFileRequest(overrides: Record<string, unknown> = {}, intent: Record<string, unknown> | null = {}) {
    vi.stubEnv('CHROME', '')
    vi.stubEnv('FIREFOX', 'true')
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    await flush()
    const details = { requestId: 'download-request', url: 'https://site.test/export.zip?token=a%2Fb+z',
      method: 'GET', tabId: 7, frameId: 0, type: 'main_frame', documentUrl: 'https://site.test/',
      timeStamp: Date.now(), requestHeaders: [
        { name: 'Authorization', value: 'Bearer this-request' },
        { name: 'Cookie', value: 'sid=this-request' },
        { name: 'Referer', value: 'https://site.test/original' },
        { name: 'Origin', value: 'https://site.test' },
        { name: 'User-Agent', value: 'Browser request UA' },
      ], ...overrides }
    item.url = item.finalUrl = details.url
    if (intent) await new Promise(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'click-intent', href: details.url, pageUrl: 'https://site.test/', ...intent },
      { url: 'https://site.test/', tab: { id: 7 }, frameId: 0 }, resolve,
    ))
    listener(mocks.browser.webRequest.onBeforeRequest)(details)
    return { blocked: listener(mocks.browser.webRequest.onBeforeSendHeaders)(details) }
  }

  it('hands a Firefox attachment to the desktop before sending its original request with exact credentials', async () => {
    local.authorizedCookieHosts = ['site.test']
    mocks.browser.cookies.getAll.mockResolvedValue([{ name: 'sid', value: 'this-request', domain: 'site.test',
      path: '/', hostOnly: true, secure: true }])
    const { blocked } = await firefoxFileRequest()
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toEqual({ cancel: true })
    const payload = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource
    expect(payload.url).toBe(item.url)
    expect(payload.cookie).toBe('sid=this-request')
    expect(payload.request_headers).toMatchObject({ authorization: 'Bearer this-request',
      referer: 'https://site.test/original', origin: 'https://site.test', 'user-agent': 'Browser request UA' })
    expect(payload.request_contexts['https://site.test'].url_contexts[item.url].request_headers.authorization).toBe('Bearer this-request')
    expect(payload.request_body).toBeUndefined()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
  })

  it.each([{ enabled: false }, { minimumBytes: 1024 }])('keeps Firefox pre-request navigation under policy %j', async config => {
    Object.assign(local, config)
    const { blocked } = await firefoxFileRequest({ requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it.each([{ method: 'POST' }, { type: 'xmlhttprequest' }, { url: 'https://site.test/video.mp4' },
    { proxyInfo: { type: 'socks', host: '127.0.0.1', port: 1080 } }])('preserves Firefox requests outside explicit file GET takeover: %j', async details => {
    const { blocked } = await firefoxFileRequest({ ...details, requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it('does not issue an original Firefox request or a second offer when its pre-request receipt is uncertain', async () => {
    let recovered = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true }
      if (message.op === 'handoff_status' && recovered) return { ok: true, handoff: { id: 'recovered-file', status: 'pending' } }
      throw new Error('receipt unavailable')
    })
    for (const requestId of ['first', 'second']) {
      const { blocked } = await firefoxFileRequest({ requestId, requestHeaders: [] })
      await vi.advanceTimersByTimeAsync(1500)
      expect(await blocked).toEqual({ cancel: true })
    }
    const originalId = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource.client_request_id
    recovered = true
    const { blocked } = await firefoxFileRequest({ requestId: 'third', requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'handoff_status' && message.client_request_id === originalId)).toBe(true)
  })

  it('leaves a refused Firefox pre-request handoff with the browser through response and DownloadItem events', async () => {
    mocks.native.mockImplementation(async message => message.op === 'ping'
      ? { ok: true } : { ok: false, error: 'desktop refused' })
    const { blocked } = await firefoxFileRequest({ requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    const received = response(true)
    await vi.advanceTimersByTimeAsync(1500)
    await received
    const created = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(1500)
    await created
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
  })

  it.each([null, { altBypass: true }, { href: 'https://site.test/another.zip' }])('requires the exact eligible Firefox file click: %j', async intent => {
    const { blocked } = await firefoxFileRequest({ requestHeaders: [] }, intent)
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it.each([false, true])('preserves Firefox request cookies when export cannot match, authorized=%s', async authorized => {
    if (authorized) local.authorizedCookieHosts = ['site.test']
    mocks.browser.cookies.getAll.mockResolvedValue([{ name: 'sid', value: 'different', domain: 'site.test',
      path: '/', hostOnly: true, secure: true }])
    const { blocked } = await firefoxFileRequest()
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it('uses one Firefox cookie snapshot from request verification through handoff', async () => {
    local.authorizedCookieHosts = ['site.test']
    let reads = 0
    mocks.browser.cookies.getAll.mockImplementation(async query => query.url && !query.partitionKey ? [{
      name: 'sid', value: ++reads === 1 ? 'this-request' : 'rotated', domain: 'site.test',
      path: '/', hostOnly: true, secure: true,
    }] : [])
    const { blocked } = await firefoxFileRequest()
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toEqual({ cancel: true })
    const payload = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource
    expect(payload.cookie).toBe('sid=this-request')
    expect(payload.request_contexts['https://site.test'].cookies[0].value).toBe('this-request')
    expect(reads).toBe(1)
  })

  it('resolves a Firefox pre-request journal after worker reload even while the desktop is offline', async () => {
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      throw new Error('receipt lost')
    })
    const first = await firefoxFileRequest({ requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await first.blocked).toEqual({ cancel: true })
    const requestId = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource.client_request_id
    vi.clearAllTimers()
    vi.resetModules()
    mocks.native.mockImplementation(async () => { throw new Error('desktop offline') })
    const retry = await firefoxFileRequest({ requestId: 'retry', requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await retry.blocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'handoff_status' && message.client_request_id === requestId)).toBe(true)
  })

  it('keeps an unsent Firefox request with the browser when the desktop is initially offline', async () => {
    mocks.native.mockImplementation(async () => { throw new Error('desktop offline') })
    const { blocked } = await firefoxFileRequest({ requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it.each(['missing click', 'disabled', 'changed cookies'])('keeps unresolved Firefox ownership when a later request has %s', async change => {
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      throw new Error('receipt lost')
    })
    const first = await firefoxFileRequest({ requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await first.blocked).toEqual({ cancel: true })
    const original = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource.client_request_id
    if (change === 'disabled') local.enabled = false
    const retry = await firefoxFileRequest({ requestId: 'second',
      requestHeaders: change === 'changed cookies' ? [{ name: 'Cookie', value: 'changed=1' }] : [] },
    change === 'missing click' ? null : {})
    await vi.advanceTimersByTimeAsync(1500)
    expect(await retry.blocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'handoff_status'
      && message.client_request_id === original).length).toBeGreaterThanOrEqual(2)
  })

  it('keeps a repeated Firefox navigation blocked while its first credential snapshot is still being prepared', async () => {
    local.authorizedCookieHosts = ['site.test']
    let release!: (value: unknown[]) => void
    const snapshot = new Promise<unknown[]>(resolve => { release = resolve })
    mocks.browser.cookies.getAll.mockImplementation(() => snapshot)
    const first = await firefoxFileRequest()
    await flush()
    const second = await firefoxFileRequest({ requestId: 'second' }, null)
    await vi.advanceTimersByTimeAsync(1500)
    release([{ name: 'sid', value: 'this-request', domain: 'site.test', path: '/', hostOnly: true, secure: true }])
    await vi.advanceTimersByTimeAsync(1500)
    expect(await first.blocked).toEqual({ cancel: true })
    expect(await second.blocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
  })

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

  it('shares only an in-flight readiness check and checks again after desktop recovery', async () => {
    let release!: (value: unknown) => void
    mocks.native.mockImplementation(async message => message.op === 'ping'
      ? new Promise(resolve => { release = resolve }) : { ok: false })
    const clicks = Promise.all([directLink(), directLink('https://site.test/another.zip')])
    await vi.advanceTimersByTimeAsync(500)
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'ping' && message.interactive)).toHaveLength(1)
    release({ ok: false })
    expect((await clicks).every(reply => reply.browserFallback === true)).toBe(true)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    mocks.native.mockImplementation(async message => message.op === 'ping'
      ? { ok: true } : { ok: true, handoff: { id: 'recovered', status: 'pending' } })
    const retry = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect(await retry).toMatchObject({ browserFallback: false, handoff: { id: 'recovered' } })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'ping' && message.interactive)).toHaveLength(2)
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

  it('uses one scoped cookie snapshot for the payload and request context', async () => {
    local.authorizedCookieHosts = ['site.test']
    let reads = 0
    mocks.browser.cookies.getAll.mockImplementation(async query => query.url && !query.partitionKey ? [{
      name: 'sid', value: ++reads === 1 ? 'original' : 'rotated',
      domain: 'site.test', path: '/', hostOnly: true, secure: true,
    }] : [])
    const click = directLink()
    await vi.advanceTimersByTimeAsync(500)
    expect((await click).ok).toBe(true)
    const payload = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource
    expect(payload.cookie).toBe('sid=original')
    expect(payload.request_contexts['https://site.test'].cookie).toBe(payload.cookie)
    expect(payload.request_contexts['https://site.test'].cookies[0].value).toBe('original')
    expect(reads).toBe(1)
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

  it('imports the finalized browser filename when it changes after onCreated', async () => {
    item.state = 'in_progress'
    item.filename = 'C:\\Downloads\\image.bin'
    response(true)
    const created = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await created
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)

    item.state = 'complete'
    item.filename = 'C:\\Downloads\\image.png'
    listener(mocks.browser.downloads.onChanged)({ id: item.id, state: { current: 'complete' } })
    await flush()
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file').map(([message]) => message))
      .toMatchObject([{ resource: { local_source_path: item.filename } }])
    expect(local['local-browser-download-handoffs']).toMatchObject([{ sourceFilename: item.filename }])
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
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'ping' && message.interactive)).toHaveLength(1)
  })

  it.each([0, 3])('hands off audio, video and key identities only from frame %s and successful requests', async frameId => {
    const urls = ['https://cdn.test/media/main.m3u8', 'https://cdn.test/media/play?track=audio',
      'https://cdn.test/media/play?track=video', 'https://cdn.test/media/key']
    for (const [index, url] of urls.entries()) {
      const request = { requestId: String(index), url, method: 'GET', tabId: 7, frameId,
        type: 'xmlhttprequest', documentUrl: 'https://site.test/', timeStamp: Date.now() + index }
      listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
        requestHeaders: [{ name: 'Authorization', value: `Bearer track-${index}` },
          { name: 'Referer', value: 'https://site.test/' }] })
      listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: index === 0 ? 'application/vnd.apple.mpegurl' : 'application/octet-stream' }] })
    }
    for (const [index, url] of [...urls, 'https://other-frame.test/media/key'].entries()) {
      const request = { requestId: `other-frame-${index}`, url, method: 'GET', tabId: 7,
        frameId: frameId + 1, type: 'xmlhttprequest', documentUrl: 'https://site.test/', timeStamp: Date.now() + 100 + index }
      listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
        requestHeaders: [{ name: 'Authorization', value: 'Bearer other-frame' }] })
      listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'application/octet-stream' }] })
    }
    const denied = { requestId: 'denied-audio', url: urls[1], method: 'GET', tabId: 7, frameId,
      type: 'xmlhttprequest', documentUrl: 'https://site.test/', timeStamp: Date.now() + 200 }
    listener(mocks.browser.webRequest.onSendHeaders)({ ...denied,
      requestHeaders: [{ name: 'Authorization', value: 'Bearer denied' }] })
    listener(mocks.browser.webRequest.onHeadersReceived)({ ...denied, statusCode: 403,
      responseHeaders: [{ name: 'Content-Type', value: 'application/json' }] })
    const sending = new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'download-now', resource: { id: 'main', url: urls[0], kind: 'hls', seenAt: Date.now() } },
      { url: 'https://site.test/', tab: { id: 7 }, frameId }, resolve,
    ))
    await vi.advanceTimersByTimeAsync(1000)
    expect((await sending).ok).toBe(true)
    const payload = mocks.native.mock.calls.find(([message]) => message.op === 'offer')?.[0].resource
    expect(payload.request_headers.authorization).toBe('Bearer track-0')
    const scoped = payload.request_contexts['https://cdn.test']
    expect(payload.request_contexts['https://other-frame.test']).toBeUndefined()
    expect(Object.keys(scoped.url_contexts).sort()).toEqual([...urls].sort())
    for (const [index, url] of urls.entries()) {
      expect(scoped.url_contexts[url].request_headers.authorization).toBe(`Bearer track-${index}`)
    }
    expect(scoped.path_contexts['/media/'].request_headers.authorization).toBe('Bearer track-0')
  })

  it('keeps Firefox early takeover bound to the original expiring signature', async () => {
    vi.stubEnv('CHROME', '')
    vi.stubEnv('FIREFOX', 'true')
    const expires = Math.floor(Date.now() / 1000)
    const originalUrl = `https://site.test/list.m3u8?s=download&e=${expires + 30}`
    const observe = (requestId: string, url: string) => {
      const request = { requestId, url, method: 'GET', tabId: 7, frameId: 0,
        type: 'main_frame', documentUrl: item.referrer, timeStamp: Date.now() }
      listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
        requestHeaders: [{ name: 'X-Playback-Token', value: requestId }] })
      return listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' },
          { name: 'Content-Disposition', value: 'attachment; filename="list.m3u8"' }] })
    }
    const first = observe('download', originalUrl)
    const second = observe('player', `https://site.test/list.m3u8?s=player&e=${expires + 300}`)
    await vi.advanceTimersByTimeAsync(2000)
    expect(await first).toEqual({ cancel: true })
    await second
    const offer = mocks.native.mock.calls.find(([message]) => message.op === 'offer'
      && message.resource.url === originalUrl)?.[0]
    expect(offer).toBeDefined()
    expect(offer.resource.request_headers['x-playback-token']).toBe('download')
  })

  it.each([false, true])('keeps the exact signed manifest and headers through browser takeover, original observed=%s', async observed => {
    item.url = item.finalUrl = 'https://site.test/list.m3u8?token=download'
    item.state = 'in_progress'
    mocks.browser.downloads.cancel.mockImplementation(async () => { item.state = 'interrupted' })
    for (const [requestId, url, timeStamp] of [
      ...(observed ? [['download', item.url, Date.now()]] as const : []),
      ['player', 'https://site.test/list.m3u8?token=player', Date.now() + 1],
    ] as const) {
      const request = { requestId, url, method: 'GET', tabId: 7, frameId: 0,
        type: 'xmlhttprequest', documentUrl: item.referrer, timeStamp }
      listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
        requestHeaders: [{ name: 'X-Playback-Token', value: requestId }] })
      listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] })
    }
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(2000)
    await takeover
    const offers = mocks.native.mock.calls.filter(([message]) => message.op === 'offer')
    if (observed) {
      expect(offers).toHaveLength(1)
      expect(offers[0][0].resource.url).toBe('https://site.test/list.m3u8?token=download')
      expect(offers[0][0].resource.request_headers['x-playback-token']).toBe('download')
    } else {
      expect(offers).toHaveLength(0)
      expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
      expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    }
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

  it('keeps a Firefox manifest until a lost receipt is recovered by the original journaled request', async () => {
    vi.stubEnv('CHROME', '')
    vi.stubEnv('FIREFOX', 'true')
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.mime = 'application/vnd.apple.mpegurl'
    item.state = 'in_progress'
    let receiptAvailable = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true }
      if (message.op === 'handoff_status' && receiptAvailable) return {
        ok: true, handoff: { id: 'recovered-manifest', status: 'pending' },
      }
      throw new Error('receipt unavailable')
    })
    const blocked = response(true)
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    const offered = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0]
    expect(local['pending-direct-offers-v1']).toEqual([
      expect.objectContaining({ clientRequestId: offered.resource.client_request_id }),
    ])
    receiptAvailable = true
    const downloadId = item.id
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(1500)
    await takeover
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'handoff_status'
      && message.client_request_id === offered.resource.client_request_id)).toBe(true)
    expect(mocks.browser.downloads.cancel).toHaveBeenCalledWith(downloadId)
  })

  it('keeps a Firefox manifest in the browser if the ownership journal cannot be saved', async () => {
    vi.stubEnv('CHROME', '')
    vi.stubEnv('FIREFOX', 'true')
    item.url = item.finalUrl = 'https://site.test/list.m3u8'
    item.mime = 'application/vnd.apple.mpegurl'
    mocks.browser.storage.local.set.mockImplementation(async value => {
      if ('pending-direct-offers-v1' in value) throw new Error('journal unavailable')
      Object.assign(local, value)
    })
    const blocked = response(true)
    await vi.advanceTimersByTimeAsync(1500)
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
