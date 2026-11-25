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

  it('hands an exact multi-part raw POST to Core through the registered resource download path', async () => {
    const url = 'https://cdn.test/export?sig=a%2Fb+z'
    const body = new TextEncoder().encode('{"asset":"中文","token":"once"}')
    const request = { requestId: 'split-post', url, method: 'POST', tabId: 7, frameId: 3,
      type: 'xmlhttprequest', documentUrl: 'https://player.test/embed', timeStamp: Date.now() }
    listener(mocks.browser.webRequest.onBeforeRequest)({ ...request,
      requestBody: { raw: [{ bytes: body.subarray(0, 12) }, { bytes: body.subarray(12) }] } })
    listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
      requestHeaders: [{ name: 'Content-Type', value: 'application/json' },
        { name: 'Authorization', value: 'Bearer exact-post' }] })
    listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
      responseHeaders: [{ name: 'Content-Type', value: 'video/mp4' }, { name: 'ETag', value: '"post-video"' }] })
    await resourceMessage({ url, kind: 'media', method: 'POST', responseIdentity: { etag: '"post-video"' } })
    await flush()
    const stored = (session[pageResourceKey(7, 'https://site.test/')] as any[]).find(resource => resource.url === url)
    const sending = new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'download-now', resource: stored }, { url: 'https://player.test/embed', tab: { id: 7 }, frameId: 3 }, resolve,
    ))
    await vi.advanceTimersByTimeAsync(1000)
    expect((await sending).ok).toBe(true)
    const payload = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource
    expect(payload.url).toBe(url)
    expect(payload.request_method).toBe('POST')
    expect(Uint8Array.from(atob(payload.request_body), char => char.charCodeAt(0))).toEqual(body)
    expect(payload.request_headers.authorization).toBe('Bearer exact-post')
    expect(payload.request_contexts['https://cdn.test'].url_contexts[url].request_headers.authorization).toBe('Bearer exact-post')
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

  function response(early = false, method = 'GET', extraHeaders: Array<{ name: string, value: string }> = []) {
    const request = { requestId: 'download-request', url: item.url, method, tabId: 7,
      frameId: 0, type: 'main_frame', documentUrl: item.referrer, timeStamp: Date.now() }
    listener(mocks.browser.webRequest.onBeforeRequest)({ ...request,
      ...(method === 'POST' ? { requestBody: { formData: { export: ['once'] } } } : {}) })
    if (method === 'POST') listener(mocks.browser.webRequest.onSendHeaders)({ ...request,
      requestHeaders: [{ name: 'Content-Type', value: 'application/x-www-form-urlencoded' }] })
    return listener(mocks.browser.webRequest.onHeadersReceived)({ ...request, statusCode: 200,
      responseHeaders: [{ name: 'Content-Type', value: 'application/octet-stream' },
        ...(early ? [{ name: 'Content-Disposition', value: 'attachment; filename="export.bin"' }] : []),
        { name: 'Content-Length', value: '1024' },
        ...extraHeaders] })
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

  it.each([
    { url: 'https://site.test/export.zip?token=a%2Fb+z', intent: {} },
    { url: 'https://site.test/export?token=a%2Fb+z', intent: { downloadAttribute: true, filename: '报表 2026.csv' } },
  ])('hands a Firefox download to the desktop before sending its original request with exact credentials: $url', async ({ url, intent }) => {
    local.authorizedCookieHosts = ['site.test']
    mocks.browser.cookies.getAll.mockResolvedValue([{ name: 'sid', value: 'this-request', domain: 'site.test',
      path: '/', hostOnly: true, secure: true }])
    const { blocked } = await firefoxFileRequest({ url }, intent)
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toEqual({ cancel: true })
    const payload = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource
    expect(payload.url).toBe(item.url)
    expect(payload.cookie).toBe('sid=this-request')
    expect(payload.request_headers).toMatchObject({ authorization: 'Bearer this-request',
      referer: 'https://site.test/original', origin: 'https://site.test', 'user-agent': 'Browser request UA' })
    expect(payload.request_contexts['https://site.test'].url_contexts[item.url].request_headers.authorization).toBe('Bearer this-request')
    if ('filename' in intent) expect(payload.filename).toBe(intent.filename)
    expect(payload.request_body).toBeUndefined()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
  })

  it.each([
    { url: 'https://site.test/export', intent: {} },
    { url: 'https://other.test/export', intent: { downloadAttribute: true } },
    { url: 'https://site.test/export', intent: { downloadAttribute: true, altBypass: true } },
    { url: 'https://site.test/export', intent: { downloadAttribute: true, href: 'https://site.test/another-export' } },
  ])('keeps extensionless Firefox navigation without an eligible same-origin download link in the browser: %j', async ({ url, intent }) => {
    const { blocked } = await firefoxFileRequest({ url, requestHeaders: [] }, intent)
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it('does not block ordinary Firefox navigation when the download journal cannot be read', async () => {
    mocks.browser.storage.local.get.mockImplementation(async key => {
      if (key === 'pending-direct-offers-v1') throw new Error('journal unavailable')
      return { ...local }
    })
    const { blocked } = await firefoxFileRequest({ url: 'https://site.test/settings', requestHeaders: [] }, null)
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it.each([{ enabled: false }, { minimumBytes: 1024 }])('keeps Firefox pre-request navigation under policy %j', async config => {
    Object.assign(local, config)
    const { blocked } = await firefoxFileRequest({ requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it.each([{ method: 'POST' }, { type: 'xmlhttprequest' }, { url: 'https://site.test/watch' },
    { proxyInfo: { type: 'socks', host: '127.0.0.1', port: 1080 } }])('preserves Firefox requests outside explicit file GET takeover: %j', async details => {
    const { blocked } = await firefoxFileRequest({ ...details, requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1500)
    expect(await blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it.each([
    { url: 'https://site.test/export.zip', intent: {} },
    { url: 'https://site.test/export', intent: { downloadAttribute: true, filename: 'report.csv' } },
  ])('does not issue an original Firefox request or a second offer when its pre-request receipt is uncertain: $url', async ({ url, intent }) => {
    let recovered = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer') return { ok: false, transport_error: true }
      if (message.op === 'handoff_status' && recovered) return { ok: true, handoff: { id: 'recovered-file', status: 'pending' } }
      throw new Error('receipt unavailable')
    })
    for (const requestId of ['first', 'second']) {
      const { blocked } = await firefoxFileRequest({ url, requestId, requestHeaders: [] }, intent)
      await vi.advanceTimersByTimeAsync(1500)
      expect(await blocked).toEqual({ cancel: true })
    }
    const originalId = mocks.native.mock.calls.find(([message]) => message.op === 'offer')![0].resource.client_request_id
    recovered = true
    const { blocked } = await firefoxFileRequest({ url, requestId: 'third', requestHeaders: [] }, null)
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
    const originalResponse = response(true, 'GET', [{ name: 'Cache-Control', value: 'no-store' }])
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
    response(true, 'GET', [{ name: 'Cache-Control', value: 'no-store' }])
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
    response(true, 'GET', [{ name: 'Cache-Control', value: 'no-store' }])
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
    response(true, 'GET', [{ name: 'Cache-Control', value: 'no-store' }])
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
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(true)
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

  function largeResumableResponse(options: {
    early?: boolean
    method?: string
    size?: number
    acceptRanges?: string
    etag?: string
    lastModified?: string
    cacheControl?: string
    cookieHeader?: string
  } = {}) {
    const {
      early = true,
      method = 'GET',
      size = 50 * 1024 * 1024,
      acceptRanges = 'bytes',
      etag = '"large-v1"',
      lastModified,
      cacheControl,
      cookieHeader,
    } = options
    const request = {
      requestId: 'accel-request',
      url: item.url,
      method,
      tabId: 7,
      frameId: 0,
      type: 'main_frame',
      documentUrl: item.referrer,
      timeStamp: Date.now(),
    }
    listener(mocks.browser.webRequest.onBeforeRequest)({
      ...request,
      ...(method === 'POST' ? { requestBody: { formData: { export: ['once'] } } } : {}),
    })
    listener(mocks.browser.webRequest.onSendHeaders)({
      ...request,
      requestHeaders: [
        ...(cookieHeader ? [{ name: 'Cookie', value: cookieHeader }] : []),
        { name: 'Referer', value: item.referrer },
      ],
    })
    const responseHeaders = [
      { name: 'Content-Type', value: 'application/octet-stream' },
      ...(early ? [{ name: 'Content-Disposition', value: 'attachment; filename="ubuntu.iso"' }] : []),
      ...(acceptRanges ? [{ name: 'Accept-Ranges', value: acceptRanges }] : []),
      { name: 'Content-Length', value: String(size) },
      ...(etag ? [{ name: 'ETag', value: etag }] : []),
      ...(lastModified ? [{ name: 'Last-Modified', value: lastModified }] : []),
      ...(cacheControl ? [{ name: 'Cache-Control', value: cacheControl }] : []),
    ]
    return listener(mocks.browser.webRequest.onHeadersReceived)({
      ...request,
      statusCode: 200,
      responseHeaders,
    })
  }

  it.each([true, false])('accelerates a large resumable GET in Core via offer instead of local copy (early=%s)', async early => {
    item.url = item.finalUrl = 'https://cdn.test/releases/ubuntu.iso'
    item.filename = 'C:\\Downloads\\ubuntu.iso'
    item.state = 'in_progress'
    item.canResume = false
    item.totalBytes = 50 * 1024 * 1024
    item.fileSize = 50 * 1024 * 1024
    mocks.browser.downloads.cancel.mockImplementation(async () => { item.state = 'interrupted' })
    mocks.native.mockImplementation(async message => {
      if (message.op === 'offer') {
        expect(item.paused).toBe(true)
        return { ok: true, handoff: { id: 'accel-handoff', status: 'pending' } }
      }
      return { ok: message.op === 'ping' }
    })

    largeResumableResponse({ early })
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()

    const offers = mocks.native.mock.calls.filter(([message]) => message.op === 'offer')
    expect(offers).toHaveLength(1)
    expect(offers[0][0].resource.url).toBe('https://cdn.test/releases/ubuntu.iso')
    expect(offers[0][0].resource.size).toBe(50 * 1024 * 1024)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.pause).toHaveBeenCalledWith(42)
    expect(mocks.browser.downloads.cancel).toHaveBeenCalledWith(42)
    expect(mocks.browser.downloads.erase).toHaveBeenCalledWith({ id: 42 })
  })

  it.each([
    { label: 'no-store without validator', overrides: { etag: '', cacheControl: 'private, no-store' } },
    { label: 'POST method', overrides: { method: 'POST' } },
    { label: 'unauthorized request cookies', overrides: { cookieHeader: 'sid=secret' } },
  ])('falls back to preserveBrowserDownload (offer_local_file) for $label', async ({ overrides }) => {
    item.url = item.finalUrl = 'https://cdn.test/releases/ubuntu.iso'
    item.filename = 'C:\\Downloads\\ubuntu.iso'
    item.state = 'in_progress'
    item.totalBytes = 50 * 1024 * 1024
    item.fileSize = item.totalBytes

    largeResumableResponse({ early: true, ...overrides })
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()

    item.state = 'complete'
    listener(mocks.browser.downloads.onChanged)({ id: 42, state: { current: 'complete' } })
    await flush()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(true)
  })

  it('accelerates active Chrome GET downloads with canResume=false, no ETag, or missing Accept-Ranges directly via offer', async () => {
    const expectedUrl = 'https://release-assets.githubusercontent.com/12345/app.zip?X-Amz-Signature=deadbeef&X-Amz-Expires=300'
    item.url = item.finalUrl = expectedUrl
    item.referrer = 'https://github.com/org/repo/releases'
    item.filename = 'C:\\Downloads\\app.zip'
    item.state = 'in_progress'
    item.canResume = false
    item.totalBytes = 64 * 1024
    item.fileSize = 64 * 1024
    mocks.browser.downloads.cancel.mockImplementation(async () => { if (item) item.state = 'interrupted' })
    // Referrer github.com has cookies, while CDN request sent NO Cookie header.
    mocks.browser.cookies.getAll.mockImplementation(async query =>
      String(query.url || '').includes('github.com')
        ? [{ name: 'user_session', value: 'gh-cookie', domain: 'github.com', path: '/', hostOnly: true, secure: true }]
        : [],
    )

    largeResumableResponse({ early: true, size: 64 * 1024, acceptRanges: '', etag: '' })
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()

    const offers = mocks.native.mock.calls.filter(([message]) => message.op === 'offer')
    expect(offers).toHaveLength(1)
    expect(offers[0][0].resource.url).toBe(expectedUrl)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.cancel).toHaveBeenCalledWith(42)
  })

  it('does not block pre-click CDN link interception because topPageUrl holds cookies', async () => {
    const cdnHref = 'https://cas-bridge.xethub.hf.co/xet-bridge-us/model.safetensors?X-Amz-Signature=abcd'
    mocks.browser.cookies.getAll.mockImplementation(async query => {
      if (query.partitionKey?.topLevelSite === 'https://huggingface.co') {
        return [{ name: 'hf_token', value: 'secret', domain: 'huggingface.co', path: '/', hostOnly: true, secure: true }]
      }
      return []
    })
    ;(mocks.browser.cookies as Record<string, unknown>).getPartitionKey = vi.fn().mockRejectedValue(
      new Error('No host permissions for cookies at url: "https://huggingface.co/".'),
    )
    try {
      const reply = await new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
        { type: 'intercept-download-link', href: cdnHref, filename: 'model.safetensors' },
        { url: 'https://huggingface.co/org/model', tab: { id: 7 }, frameId: 0 },
        resolve,
      ))
      expect(reply.ok).toBe(true)
      expect(reply.browserFallback).toBe(false)
      expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
    } finally {
      delete (mocks.browser.cookies as Record<string, unknown>).getPartitionKey
    }
  })

  it('leaves small downloads completely untouched in the browser when minimumBytes > 0 without queuing local handoffs', async () => {
    local.minimumBytes = 10 * 1024 * 1024
    item.url = item.finalUrl = 'https://cdn.test/small-doc.pdf'
    item.filename = 'C:\\Downloads\\small-doc.pdf'
    item.state = 'in_progress'
    item.totalBytes = 512 * 1024
    item.fileSize = 512 * 1024

    largeResumableResponse({ early: true, size: 512 * 1024 })
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()

    expect(mocks.native.mock.calls.some(([message]) => ['offer', 'offer_local_file'].includes(message.op))).toBe(false)
    expect(local['local-browser-download-handoffs']).toBeUndefined()
    expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
  })

  it('leaves background downloads in the browser during user_shutdown while explicit popup/button download wakes Core', async () => {
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') {
        return { ok: false, code: 'core_unreachable', error_code: 'user_shutdown', user_shutdown: true, error: '已退出' }
      }
      if (message.op === 'offer') {
        return { ok: true, handoff: { id: 'woken-handoff', status: 'pending' } }
      }
      return { ok: false }
    })

    // Background POST download while user_shutdown is active: left in browser, NOT queued into localDownloadHandoffs!
    item.state = 'in_progress'
    response(false, 'POST')
    const bgPost = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await bgPost
    await flush()
    expect(mocks.native.mock.calls.some(([message]) => ['offer', 'offer_local_file'].includes(message.op))).toBe(false)
    expect(local['local-browser-download-handoffs']).toBeUndefined()

    // Explicit user click from Popup / floating button (`download-now`) wakes Core directly via `offer`!
    const explicitReply = await new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      {
        type: 'download-now',
        resource: { id: 'vid-1', url: 'https://cdn.test/video.mp4', kind: 'media', pageUrl: 'https://site.test/', seenAt: Date.now() },
      },
      { url: 'https://site.test/', tab: { id: 7 }, frameId: 0 },
      resolve,
    ))
    expect(explicitReply).toMatchObject({ ok: true, handoff: { id: 'woken-handoff' } })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)
  })

  it('accelerates an authenticated large resumable GET when authorized cookies match exactly', async () => {
    local.authorizedCookieHosts = ['cdn.test']
    mocks.browser.cookies.getAll.mockResolvedValue([{
      name: 'sid', value: 'auth-ok', domain: 'cdn.test', path: '/', hostOnly: true, secure: true,
    }])
    item.url = item.finalUrl = 'https://cdn.test/releases/ubuntu.iso'
    item.filename = 'C:\\Downloads\\ubuntu.iso'
    item.state = 'in_progress'
    item.totalBytes = 50 * 1024 * 1024
    item.fileSize = 50 * 1024 * 1024
    mocks.browser.downloads.cancel.mockImplementation(async () => { item.state = 'interrupted' })

    largeResumableResponse({ early: true, cookieHeader: 'sid=auth-ok' })
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover

    const offered = mocks.native.mock.calls.find(([message]) => message.op === 'offer')?.[0]
    expect(offered).toBeDefined()
    expect(offered.resource.cookie).toBe('sid=auth-ok')
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.cancel).toHaveBeenCalledWith(42)
  })

  it('falls back to local browser download import instead of abandoning takeover when minimumBytes > 0 and cookies are unauthorized', async () => {
    local.minimumBytes = 1 * 1024 * 1024
    local.authorizedCookieHosts = []
    item.url = item.finalUrl = 'https://cdn.test/releases/private-sdk.iso'
    item.filename = 'C:\\Downloads\\private-sdk.iso'
    item.state = 'in_progress'
    item.totalBytes = 50 * 1024 * 1024
    item.fileSize = 50 * 1024 * 1024

    largeResumableResponse({ early: true, size: 50 * 1024 * 1024, cookieHeader: 'sid=unauth-session' })
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()

    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
    expect(local['local-browser-download-handoffs']).toMatchObject([
      { downloadId: 42 },
    ])

    item.state = 'complete'
    listener(mocks.browser.downloads.onChanged)({ id: 42, state: { current: 'complete' } })
    await flush()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(true)
  })

  it('accelerates large resumable files in Firefox onHeadersReceived even when minimumBytes > 0', async () => {
    vi.stubEnv('CHROME', '')
    vi.stubEnv('FIREFOX', 'true')
    local.minimumBytes = 1 * 1024 * 1024
    item.url = item.finalUrl = 'https://cdn.test/releases/ubuntu.iso'
    item.state = 'in_progress'

    const largeBlocked = largeResumableResponse({ early: true, size: 50 * 1024 * 1024 })
    await vi.advanceTimersByTimeAsync(500)
    expect(await largeBlocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.filter(([message]) => message.op === 'offer')).toHaveLength(1)

    mocks.native.mockClear()
    item.url = item.finalUrl = 'https://cdn.test/releases/tiny.zip'
    const smallBlocked = largeResumableResponse({ early: true, size: 512 * 1024 })
    await vi.advanceTimersByTimeAsync(500)
    expect(await smallBlocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
  })

  it('registers and dispatches the page-links harvest context menu only for http(s) tabs', async () => {
    expect(mocks.browser.contextMenus.create).toHaveBeenCalledWith(expect.objectContaining({
      id: 'hls-download-page-links',
      title: '使用 HLS Downloader 下载页面全部链接',
      contexts: ['page'],
    }))
    mocks.browser.tabs.sendMessage.mockClear()
    listener(mocks.browser.contextMenus.onClicked)(
      { menuItemId: 'hls-download-page-links', pageUrl: 'https://files.test/index.html', frameId: 0 },
      { id: 7, url: 'https://files.test/index.html' },
    )
    expect(mocks.browser.tabs.sendMessage).toHaveBeenCalledWith(7, { type: 'collect-page-links' }, { frameId: 0 })

    mocks.browser.tabs.sendMessage.mockClear()
    listener(mocks.browser.contextMenus.onClicked)(
      { menuItemId: 'hls-download-page-links', pageUrl: 'chrome://extensions', frameId: 0 },
      { id: 7, url: 'chrome://extensions' },
    )
    expect(mocks.browser.tabs.sendMessage).not.toHaveBeenCalled()
  })

  it('syncs quickDownloadEnabled and speedLimitEnabled through set-takeover-settings', async () => {
    mocks.native.mockImplementation(async message => {
      if (message.op === 'set_takeover_settings') {
        return {
          ok: true,
          takeover_enabled: true,
          takeover_minimum_bytes: 0,
          quick_download_enabled: Boolean(message.quick_download_enabled),
          speed_limit_enabled: Boolean(message.speed_limit_enabled),
          speed_limit_kib: 256,
        }
      }
      return { ok: true }
    })
    const reply = await new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'set-takeover-settings', quickDownloadEnabled: true, speedLimitEnabled: true },
      {},
      resolve,
    ))
    await flush()
    expect(reply).toMatchObject({ ok: true, quick_download_enabled: true, speed_limit_enabled: true })
    expect(mocks.native).toHaveBeenCalledWith(expect.objectContaining({
      op: 'set_takeover_settings',
      quick_download_enabled: true,
      speed_limit_enabled: true,
    }), undefined, 0)
    expect(local.quickDownloadEnabled).toBe(true)
    expect(local.speedLimitEnabled).toBe(true)
    expect(local.speedLimitKib).toBe(256)
  })

  it('bypasses desktop takeover on Chrome Alt+Click even when webRequest tabId is -1 and mirror redirects drop referrer', async () => {
    const pageUrl = 'https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/'
    const linkUrl = `${pageUrl}debian-13.8.0-amd64-netinst.iso`
    const mirrorUrl = 'https://laotzu.ftp.acc.umu.se/debian-cd/current/amd64/iso-cd/debian-13.8.0-amd64-netinst.iso'
    mocks.browser.tabs.get.mockResolvedValue({ url: pageUrl })
    mocks.native.mockClear()

    // 1. pointerdown records Alt+Click intent from tab 7.
    await new Promise<void>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'click-intent', href: linkUrl, pageUrl, altBypass: true, ctrlForce: false, generic: false },
      { url: pageUrl, tab: { id: 7 }, frameId: 0 },
      () => resolve(),
    ))

    // 2. Chrome DownloadUrl initiates an 'other' request with tabId=-1, frameId=-1, redirecting to a mirror.
    listener(mocks.browser.webRequest.onBeforeRequest)({
      requestId: 'alt-iso', url: linkUrl, method: 'GET', tabId: -1, frameId: -1,
      type: 'other', initiator: 'https://cdimage.debian.org', timeStamp: Date.now(),
    })
    listener(mocks.browser.webRequest.onBeforeRedirect)({
      requestId: 'alt-iso', url: linkUrl, redirectUrl: mirrorUrl, method: 'GET',
      tabId: -1, frameId: -1, type: 'other', statusCode: 302, timeStamp: Date.now(),
    })
    listener(mocks.browser.webRequest.onHeadersReceived)({
      requestId: 'alt-iso', url: mirrorUrl, method: 'GET', tabId: -1, frameId: -1,
      type: 'other', statusCode: 200, timeStamp: Date.now(),
      responseHeaders: [
        { name: 'Content-Type', value: 'application/x-iso9660-image' },
        { name: 'Content-Length', value: String(650 * 1024 * 1024) },
        { name: 'Accept-Ranges', value: 'bytes' },
      ],
    })

    // 3. Chrome fires downloads.onCreated without tabId and with origin-only referrer.
    item = {
      id: 99, url: linkUrl, finalUrl: mirrorUrl, referrer: 'https://cdimage.debian.org/',
      filename: 'C:\\Downloads\\debian-13.8.0-amd64-netinst.iso', mime: 'application/x-iso9660-image',
      state: 'in_progress', paused: false, bytesReceived: 0, totalBytes: 650 * 1024 * 1024, fileSize: 650 * 1024 * 1024,
      startTime: '2026-10-29T00:00:00.000Z',
    }
    const created = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(1_000)
    await created

    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
    expect(mocks.browser.downloads.cancel).not.toHaveBeenCalled()
  })

  it('preserves Alt+Click bypass across onHeadersReceived early takeover even if request chain is lost before onCreated', async () => {
    const pageUrl = 'https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/'
    const linkUrl = `${pageUrl}debian-13.8.0-amd64-netinst.iso`
    mocks.browser.tabs.get.mockResolvedValue({ url: pageUrl })
    mocks.native.mockClear()

    await new Promise<void>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'click-intent', href: linkUrl, pageUrl, altBypass: true, ctrlForce: false, generic: false },
      { url: pageUrl, tab: { id: 7 }, frameId: 0 },
      () => resolve(),
    ))

    // Early main_frame headers observe the ISO response and peek altBypass.
    listener(mocks.browser.webRequest.onHeadersReceived)({
      requestId: 'early-alt-iso', url: linkUrl, method: 'GET', tabId: 7, frameId: 0,
      type: 'main_frame', documentUrl: pageUrl, statusCode: 200, timeStamp: Date.now(),
      responseHeaders: [
        { name: 'Content-Type', value: 'application/octet-stream' },
        { name: 'Content-Disposition', value: 'attachment; filename="debian-13.8.0-amd64-netinst.iso"' },
      ],
    })
    await flush()

    // Simulate tab navigation clearing requestChains before onCreated fires (with no DownloadItem.tabId).
    listener(mocks.browser.tabs.onUpdated)(7, { status: 'loading' })
    item = {
      id: 100, url: linkUrl, finalUrl: linkUrl, referrer: '',
      filename: 'C:\\Downloads\\debian-13.8.0-amd64-netinst.iso', mime: 'application/octet-stream',
      state: 'in_progress', paused: false, bytesReceived: 0, totalBytes: 650 * 1024 * 1024, fileSize: 650 * 1024 * 1024,
      startTime: '2026-10-29T00:00:00.000Z',
    }
    const created = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(1_000)
    await created

    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer')).toBe(false)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'offer_local_file')).toBe(false)
    expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
  })

  it('handles offline Core ping cleanly without retry-spamming or pushing pending takeover settings', async () => {
    local['pending-takeover-settings-v1'] = { id: 'pending-1', enabled: false }
    mocks.native.mockClear()
    mocks.native.mockResolvedValue({ ok: false, error: 'Core is offline' })

    const reply = await new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'ping' },
      {},
      resolve,
    ))
    await flush()

    expect(reply).toEqual({ ok: false, error: 'Core is offline' })
    expect(mocks.native).toHaveBeenCalledTimes(1)
    // Passive ping must not request interactive/auto-start or retries.
    expect(mocks.native.mock.calls[0][0]).toMatchObject({ op: 'ping' })
    expect(mocks.native.mock.calls[0][0].interactive).toBeUndefined()
    expect(mocks.native.mock.calls[0][2]).toBe(0)
    expect(mocks.native.mock.calls.some(([message]) => message.op === 'set_takeover_settings')).toBe(false)
  })

  it('falls back to top frame when collect-page-links fails in a subframe', async () => {
    mocks.browser.tabs.sendMessage.mockClear()
    mocks.browser.tabs.sendMessage
      .mockRejectedValueOnce(new Error('Could not establish connection'))
      .mockResolvedValueOnce({ ok: true, count: 3 })

    listener(mocks.browser.contextMenus.onClicked)(
      { menuItemId: 'hls-download-page-links', pageUrl: 'https://cdimage.debian.org/iso-cd/', frameId: 5 },
      { id: 7, url: 'https://cdimage.debian.org/iso-cd/' },
    )
    await flush()

    expect(mocks.browser.tabs.sendMessage).toHaveBeenNthCalledWith(1, 7, { type: 'collect-page-links' }, { frameId: 5 })
    expect(mocks.browser.tabs.sendMessage).toHaveBeenNthCalledWith(2, 7, { type: 'collect-page-links' }, { frameId: 0 })
  })

  it('wakes Core directly on explicit download-now and download-batch even when ping reports user_shutdown, forwarding audio_url', async () => {
    const pageUrl = 'https://www.bilibili.com/video/BV1xx411c7mD/'
    const videoUrl = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30080.m4s?deadline=1900000000'
    const audioUrl = 'https://upos-hz-mirrorakam.akamaized.net/upgcxcode/1/2/123-1-30280.m4s?deadline=1900000000'
    mocks.browser.tabs.get.mockResolvedValue({ url: pageUrl })
    mocks.native.mockClear()
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: false, error_code: 'user_shutdown', error: 'user_shutdown' }
      if (message.op === 'offer') return { ok: true, handoff: { id: `handoff-${message.resource.client_request_id}`, status: 'pending' } }
      if (message.op === 'handoff_status') return { ok: true, handoff: { id: String(message.handoff_id || 'handoff'), status: 'accepted', task_status: 'paused', task_error_code: 'refresh_armed' } }
      return { ok: true }
    })

    const resource = {
      id: 'bili-dual',
      url: videoUrl,
      audioUrl,
      kind: 'media',
      mimeType: 'video/mp4',
      pageUrl,
      title: 'Bilibili 4K Demo',
      filename: '100026.m4s',
      seenAt: Date.now(),
    }
    const singleReply = await new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'download-now', resource },
      { url: pageUrl, tab: { id: 7 }, frameId: 0 },
      resolve,
    ))
    await flush()
    expect(singleReply.ok).toBe(true)
    expect(mocks.native.mock.calls.some(([m]) => m.op === 'ping')).toBe(false)
    const offerPayload = mocks.native.mock.calls.find(([m]) => m.op === 'offer')![0].resource
    expect(offerPayload.url).toBe(videoUrl)
    expect(offerPayload.audio_url).toBe(audioUrl)
    expect(offerPayload.filename).toBe('Bilibili 4K Demo.mp4')
    expect(offerPayload.request_contexts['https://upos-hz-mirrorakam.akamaized.net']).toBeDefined()

    // Explicit forceResend rotates client_request_id for ArmTaskRefresh / re-download.
    const resendReply = await new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'download-now', resource, forceResend: true },
      { url: pageUrl, tab: { id: 7 }, frameId: 0 },
      resolve,
    ))
    await flush()
    expect(resendReply.ok).toBe(true)
    const offers = mocks.native.mock.calls.filter(([m]) => m.op === 'offer')
    expect(offers).toHaveLength(2)
    expect(offers[1][0].resource.client_request_id).not.toBe(offers[0][0].resource.client_request_id)

    // download-batch processes multiple items in background without popup-lifetime drop.
    mocks.native.mockClear()
    const batchReply = await new Promise<any>(resolve => listener(mocks.browser.runtime.onMessage)(
      {
        type: 'download-batch',
        resources: [
          { id: 'b1', url: 'https://files.test/one.zip', kind: 'file', pageUrl },
          { id: 'b2', url: 'https://files.test/two.zip', kind: 'file', pageUrl },
        ],
      },
      { url: pageUrl, tab: { id: 7 }, frameId: 0 },
      resolve,
    ))
    await flush()
    expect(batchReply).toMatchObject({ ok: true, sent: 2, failed: 0 })
    expect(mocks.native.mock.calls.filter(([m]) => m.op === 'offer')).toHaveLength(2)
  })

  it('migrates captured tab resources across same-video SPA replaceState redirects', async () => {
    const cnUrl = 'https://www.bilibili.com/video/BV1xx411c7mD/'
    const enUrl = 'https://www.bilibili.com/en/video/BV1xx411c7mD/?vd_source=abc'
    const cnKey = pageResourceKey(7, cnUrl)
    const enKey = pageResourceKey(7, enUrl)
    session[cnKey] = [{ id: 'bili', url: 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30080.m4s', kind: 'media', mimeType: 'video/mp4', evidence: ['site_player_metadata'], pageUrl: cnUrl, seenAt: Date.now() }]

    mocks.browser.tabs.get.mockResolvedValue({ url: enUrl })
    listener(mocks.browser.tabs.onUpdated)(7, { url: enUrl })
    await flush()

    expect(session[enKey]).toBeDefined()
    expect((session[enKey] as any[])[0].pageUrl).toBe(enUrl)
  })

  it('polls incomplete local-file handoffs every 5s instead of 1s spam', async () => {
    item.url = item.finalUrl = 'https://cdn.test/small-local.bin'
    item.filename = 'C:\\Downloads\\small-local.bin'
    item.state = 'in_progress'
    item.totalBytes = 128 * 1024
    item.fileSize = 128 * 1024

    largeResumableResponse({ early: true, size: 128 * 1024, acceptRanges: 'none', etag: '', cacheControl: 'no-store' })
    const created = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await created

    const searchBefore = mocks.browser.downloads.search.mock.calls.length
    // 2 seconds later (< 5_000ms retryAt), no follow-up poll should have run yet.
    await vi.advanceTimersByTimeAsync(2_000)
    expect(mocks.browser.downloads.search.mock.calls.length).toBe(searchBefore)

    // At 5_000ms, the scheduled follow-up runs once.
    await vi.advanceTimersByTimeAsync(3_100)
    expect(mocks.browser.downloads.search.mock.calls.length).toBeGreaterThan(searchBefore)
  })

  it('retains local-file handoff in queue when offer_local_file fails with non-shutdown core_unreachable (BUG-M7-1)', async () => {
    let reachable = false
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') return { ok: true }
      if (message.op === 'offer_local_file') {
        return reachable
          ? { ok: true, handoff: { id: 'local-after-unreachable', status: 'pending' } }
          : { ok: false, code: 'core_unreachable', stage: 'core', error: '下载引擎未启动或无响应' }
      }
      if (message.op === 'handoff_status') {
        return { ok: true, handoff: { id: 'local-after-unreachable', status: 'accepted', task_status: 'completed' } }
      }
      return { ok: false }
    })
    response(false, 'POST')
    const takeover = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await takeover
    await flush()

    const firstAttempt = mocks.native.mock.calls.find(([message]) => message.op === 'offer_local_file')![0]
    expect(local['local-browser-download-handoffs']).toMatchObject([
      { requestId: firstAttempt.resource.client_request_id },
    ])
    expect(mocks.browser.downloads.removeFile).not.toHaveBeenCalled()

    reachable = true
    await vi.advanceTimersByTimeAsync(60_000)
    const allAttempts = mocks.native.mock.calls.filter(([message]) => message.op === 'offer_local_file')
    expect(allAttempts).toHaveLength(2)
    expect(allAttempts[1][0].resource.client_request_id).toBe(firstAttempt.resource.client_request_id)
    expect(mocks.browser.downloads.removeFile).toHaveBeenCalledWith(42)
    expect(local['local-browser-download-handoffs']).toEqual([])
  })

  it('allows usable short-lived signed direct file URLs and falls back to browser on pre-handoff failure (BUG-M7-2)', async () => {
    const validExpires = Math.floor(Date.now() / 1000) + 600
    const expiredAt = Math.floor(Date.now() / 1000) - 60
    const validSignedMp4 = `https://mxcontent.test/v2/asset.mp4?s=sig-ok&e=${validExpires}&quality=1080`
    const expiredSignedMp4 = `https://mxcontent.test/v2/asset.mp4?s=sig-old&e=${expiredAt}&quality=1080`

    // 1. Chrome `intercept-download-link` hands usable `.mp4?s=...&e=...` directly to Core without prior response chain.
    const okReply = await directLink(validSignedMp4)
    expect(okReply).toMatchObject({ ok: true, browserFallback: false, handoff: { id: 'handoff' } })
    expect(mocks.native.mock.calls.find(([m]) => m.op === 'offer' && m.resource.url === validSignedMp4)).toBeDefined()

    // 2. Chrome `intercept-download-link` returns `browserFallback: true` on pre-offer throw (expired signature), never swallowing click.
    mocks.native.mockClear()
    const expiredReplyPromise = directLink(expiredSignedMp4)
    await vi.advanceTimersByTimeAsync(2_000)
    const expiredReply = await expiredReplyPromise
    expect(expiredReply).toMatchObject({ ok: false, browserFallback: true })
    expect(mocks.native.mock.calls.some(([m]) => m.op === 'offer')).toBe(false)

    // 3. Firefox `prepareFirefoxFileRequest` hands usable `.mp4?s=...&e=...` to Core, and falls back (`undefined`) on pre-offer throw.
    const ffValid = await firefoxFileRequest({ url: validSignedMp4, requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(1_500)
    expect(await ffValid.blocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.some(([m]) => m.op === 'offer' && m.resource.url === validSignedMp4)).toBe(true)

    mocks.native.mockClear()
    const ffExpired = await firefoxFileRequest({ url: expiredSignedMp4, requestId: 'ff-expired', requestHeaders: [] })
    await vi.advanceTimersByTimeAsync(2_000)
    expect(await ffExpired.blocked).toBeUndefined()
    expect(mocks.native.mock.calls.some(([m]) => m.op === 'offer')).toBe(false)
  })

  it('isolates incognito/container Cookie store via backfilled tabId/frameId and preserves explicit resourceCookies (BUG-M7-3)', async () => {
    local.authorizedCookieHosts = ['cdn.test', 'site.test']
    mocks.browser.cookies.getAllCookieStores.mockResolvedValue([
      { id: 'firefox-default', tabIds: [7] },
      { id: 'firefox-container-private', tabIds: [19] },
    ])
    mocks.browser.tabs.get.mockImplementation(async (tabId: number) => ({
      url: tabId === 19 ? 'https://site.test/container-room' : 'https://site.test/',
    }))
    mocks.browser.cookies.getAll.mockImplementation(async (query: Record<string, unknown>) => {
      if (query.partitionKey) return []
      if (query.storeId === 'firefox-container-private') {
        return [{ name: 'sid', value: 'container-secret', domain: 'cdn.test', path: '/', hostOnly: true, secure: true }]
      }
      // Default store holds a different profile session that MUST NOT leak!
      return [{ name: 'sid', value: 'default-profile-leak', domain: 'cdn.test', path: '/', hostOnly: true, secure: true }]
    })

    // Simulate a late-arriving / tabId=-1 webRequest chain paired with a DownloadItem carrying container tabId=19, frameId=2.
    const containerUrl = 'https://cdn.test/private/archive.zip'
    item = {
      ...item,
      id: 77,
      url: containerUrl,
      finalUrl: containerUrl,
      referrer: '',
      filename: 'C:\\Downloads\\archive.zip',
      state: 'in_progress',
      totalBytes: 20 * 1024 * 1024,
      fileSize: 20 * 1024 * 1024,
      tabId: 19,
      frameId: 2,
    }
    mocks.browser.downloads.cancel.mockImplementation(async () => { if (item) item.state = 'interrupted' })

    const req = {
      requestId: 'container-dl',
      url: containerUrl,
      method: 'GET',
      tabId: -1,
      frameId: -1,
      type: 'other',
      documentUrl: 'https://site.test/container-room',
      timeStamp: Date.now(),
    }
    listener(mocks.browser.webRequest.onBeforeRequest)(req)
    listener(mocks.browser.webRequest.onSendHeaders)({
      ...req,
      requestHeaders: [
        { name: 'Cookie', value: 'sid=container-secret' },
        { name: 'X-Container-Header', value: 'isolated' },
      ],
    })
    listener(mocks.browser.webRequest.onHeadersReceived)({
      ...req,
      statusCode: 200,
      responseHeaders: [
        { name: 'Content-Type', value: 'application/zip' },
        { name: 'Content-Length', value: String(20 * 1024 * 1024) },
        { name: 'Accept-Ranges', value: 'bytes' },
      ],
    })

    const created = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(1_000)
    await created
    await flush()

    const offered = mocks.native.mock.calls.find(([m]) => m.op === 'offer' && m.resource.url === containerUrl)?.[0]
    expect(offered).toBeDefined()
    expect(offered.resource.cookie).toBe('sid=container-secret')
    expect(offered.resource.cookie).not.toContain('default-profile-leak')
    expect(offered.resource.source_page_url).toBe('https://site.test/container-room')
    expect(offered.resource.request_headers['x-container-header']).toBe('isolated')
    expect(mocks.browser.cookies.getAll.mock.calls.every(([q]) => q.storeId === 'firefox-container-private')).toBe(true)

    // Also verify late-arriving `resolvedChain` backfills `tabId`/`frameId` when Chrome `DownloadItem` omits `tabId`.
    mocks.native.mockClear()
    mocks.browser.cookies.getAll.mockClear()
    const lateUrl = 'https://cdn.test/private/late-chain.zip'
    item = {
      id: 78,
      url: lateUrl,
      finalUrl: lateUrl,
      referrer: '',
      filename: 'C:\\Downloads\\late-chain.zip',
      mime: 'application/zip',
      state: 'in_progress',
      paused: false,
      bytesReceived: 0,
      totalBytes: 20 * 1024 * 1024,
      fileSize: 20 * 1024 * 1024,
      startTime: '2026-10-29T00:01:00.000Z',
    }
    const lateCreated = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(100)
    const lateReq = {
      requestId: 'late-container-dl',
      url: lateUrl,
      method: 'GET',
      tabId: 19,
      frameId: 2,
      type: 'main_frame',
      documentUrl: 'https://site.test/container-room',
      timeStamp: Date.now(),
    }
    listener(mocks.browser.webRequest.onBeforeRequest)(lateReq)
    listener(mocks.browser.webRequest.onSendHeaders)({
      ...lateReq,
      requestHeaders: [
        { name: 'Cookie', value: 'sid=container-secret' },
        { name: 'X-Late-Header', value: 'backfilled' },
      ],
    })
    listener(mocks.browser.webRequest.onHeadersReceived)({
      ...lateReq,
      statusCode: 200,
      responseHeaders: [
        { name: 'Content-Type', value: 'application/zip' },
        { name: 'Content-Length', value: String(20 * 1024 * 1024) },
        { name: 'Accept-Ranges', value: 'bytes' },
      ],
    })
    await vi.advanceTimersByTimeAsync(1_000)
    await lateCreated
    await flush()

    const lateOffered = mocks.native.mock.calls.find(([m]) => m.op === 'offer' && m.resource.url === lateUrl)?.[0]
    expect(lateOffered).toBeDefined()
    expect(lateOffered.resource.cookie).toBe('sid=container-secret')
    expect(lateOffered.resource.request_headers['x-late-header']).toBe('backfilled')
    expect(mocks.browser.cookies.getAll.mock.calls.every(([q]) => q.storeId === 'firefox-container-private')).toBe(true)
  })

  it('preserves ctrlForce across fallback re-emission, disabled downloads.onCreated, and Firefox interceptors (BUG-M7-4)', async () => {
    const { readFileSync } = await import('node:fs')
    const { fileURLToPath } = await import('node:url')
    const takeoverContent = readFileSync(fileURLToPath(new URL('../entrypoints/takeover.content.ts', import.meta.url)), 'utf8')
    expect(takeoverContent).toContain('ctrlForce: Boolean(ctrlForce)')

    // Disable global takeover, mute site, set high minimumBytes, and report user_shutdown on passive ping!
    local.enabled = false
    local.minimumBytes = 100 * 1024 * 1024
    local.excludedHosts = ['site.test', 'cdn.test']
    local['handoff-suppressions-v1'] = [{ host: 'site.test', kind: 'file', createdAt: Date.now() }]
    mocks.native.mockImplementation(async message => {
      if (message.op === 'ping') {
        return { ok: false, code: 'core_unreachable', error_code: 'user_shutdown', user_shutdown: true, error: '已退出' }
      }
      if (message.op === 'offer') {
        return { ok: true, handoff: { id: 'forced-handoff', status: 'pending' } }
      }
      return { ok: true }
    })

    // 1. Non-forced download while `!config.enabled` returns early WITHOUT pausing the download!
    item = {
      ...item,
      id: 88,
      url: 'https://cdn.test/normal.zip',
      finalUrl: 'https://cdn.test/normal.zip',
      referrer: 'https://site.test/',
      filename: 'C:\\Downloads\\normal.zip',
      state: 'in_progress',
      totalBytes: 1024,
      fileSize: 1024,
    }
    largeResumableResponse({ early: false, size: 1024 })
    const unforced = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(200)
    await unforced
    expect(mocks.browser.downloads.pause).not.toHaveBeenCalled()
    expect(mocks.native.mock.calls.some(([m]) => m.op === 'offer')).toBe(false)

    // 2. Forced click intent (`ctrlForce: true`) in `downloads.onCreated` (even without earlyTakeover) takes over and wakes Core!
    mocks.browser.downloads.cancel.mockImplementation(async () => { if (item) item.state = 'interrupted' })
    item = {
      ...item,
      id: 89,
      url: 'https://cdn.test/forced.zip',
      finalUrl: 'https://cdn.test/forced.zip',
      referrer: 'https://site.test/',
      filename: 'C:\\Downloads\\forced.zip',
      state: 'in_progress',
      totalBytes: 1024,
      fileSize: 1024,
    }
    await new Promise<void>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'click-intent', href: item.url, pageUrl: 'https://site.test/', altBypass: false, ctrlForce: true, generic: false },
      { url: 'https://site.test/', tab: { id: 7 }, frameId: 0 },
      () => resolve(),
    ))
    largeResumableResponse({ early: false, size: 1024 })
    const forcedCreated = listener(mocks.browser.downloads.onCreated)({ ...item })
    await vi.advanceTimersByTimeAsync(500)
    await forcedCreated
    expect(mocks.native.mock.calls.filter(([m]) => m.op === 'offer' && m.resource.url === 'https://cdn.test/forced.zip')).toHaveLength(1)

    // 3. Firefox `prepareFirefoxFileRequest` and `interceptFirefoxDownload` honor `ctrlForce: true` despite disabled/muted/minimumBytes/user_shutdown!
    mocks.native.mockClear()
    item = {
      id: 90,
      url: 'https://site.test/forced-pre.zip',
      finalUrl: 'https://site.test/forced-pre.zip',
      referrer: 'https://site.test/',
      filename: 'C:\\Downloads\\forced-pre.zip',
      mime: 'application/zip',
      state: 'in_progress',
      paused: false,
      bytesReceived: 0,
      totalBytes: 1024,
      fileSize: 1024,
      tabId: 7,
      startTime: '2026-10-29T00:02:00.000Z',
    }
    const ffPre = await firefoxFileRequest(
      { url: 'https://site.test/forced-pre.zip', requestHeaders: [] },
      { ctrlForce: true },
    )
    await vi.advanceTimersByTimeAsync(1_000)
    expect(await ffPre.blocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.filter(([m]) => m.op === 'offer' && m.resource.url === 'https://site.test/forced-pre.zip')).toHaveLength(1)

    mocks.native.mockClear()
    item.url = item.finalUrl = 'https://cdn.test/forced-resp.iso'
    await new Promise<void>(resolve => listener(mocks.browser.runtime.onMessage)(
      { type: 'click-intent', href: item.url, pageUrl: 'https://site.test/', altBypass: false, ctrlForce: true, generic: false },
      { url: 'https://site.test/', tab: { id: 7 }, frameId: 0 },
      () => resolve(),
    ))
    const ffRespBlocked = largeResumableResponse({ early: true, size: 1024 })
    await vi.advanceTimersByTimeAsync(1_000)
    expect(await ffRespBlocked).toEqual({ cancel: true })
    expect(mocks.native.mock.calls.filter(([m]) => m.op === 'offer' && m.resource.url === 'https://cdn.test/forced-resp.iso')).toHaveLength(1)
  })
})
