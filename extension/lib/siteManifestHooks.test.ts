import { afterEach, expect, it, vi } from 'vitest'

const responseDescriptors = Object.getOwnPropertyDescriptors(Response.prototype)
afterEach(() => {
  Object.defineProperties(Response.prototype, responseDescriptors)
  vi.unstubAllGlobals()
})

it('bridges fetch/XHR metadata without consuming responses or carrying observations across pages', async () => {
  const api = 'https://www.youtube.com/youtubei/v1/player'
  const manifest = 'https://manifest.googlevideo.com/api/manifest/hls_variant/id/video_1'
  const body = JSON.stringify({ videoDetails: { videoId: 'video_1' }, streamingData: { hlsManifestUrl: manifest } })
  const response = new Response(body, { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(response, 'url', { value: api })
  const clone = response.clone.bind(response)
  Object.defineProperty(response, 'clone', { value: () => {
    const copy = clone()
    Object.defineProperty(copy, 'url', { value: api })
    return copy
  } })
  const windowStub = new EventTarget() as EventTarget & { fetch: typeof fetch }
  const fetchMock = vi.fn(async () => response)
  windowStub.fetch = fetchMock
  const locationStub = { href: 'https://www.youtube.com/watch?v=video_1' }
  class Xhr extends EventTarget {
    responseURL = api
    status = 200
    responseType = ''
    mimeType = 'application/json'
    responseText = body
    response: unknown = body
    open() {}
    getResponseHeader() { return this.mimeType }
  }
  vi.stubGlobal('defineContentScript', (config: unknown) => config)
  vi.stubGlobal('window', windowStub)
  vi.stubGlobal('location', locationStub)
  vi.stubGlobal('XMLHttpRequest', Xhr)
  vi.stubGlobal('history', { pushState() {}, replaceState() {} })
  vi.stubGlobal('Element', class { attachShadow() {} })
  vi.stubGlobal('CustomEvent', class extends Event {
    detail: unknown
    constructor(type: string, init: { detail: unknown }) { super(type); this.detail = init.detail }
  })
  const events: any[] = []
  let firstReport: () => void = () => {}
  const reported = new Promise<void>(resolve => { firstReport = resolve })
  windowStub.addEventListener('__hls_downloader_resource__', event => {
    events.push(JSON.parse((event as CustomEvent).detail))
    firstReport()
  })
  const hook = await import('../entrypoints/hooks.content')
  ;(hook.default as any).main()
  expect(await windowStub.fetch(api)).toBe(response)
  await reported
  expect(response.bodyUsed).toBe(false)
  expect(await response.text()).toBe(body)
  expect(events).toEqual([{ url: manifest, mimeType: 'application/vnd.apple.mpegurl', siteVideoId: 'video_1', sourcePageUrl: locationStub.href }])

  const xhr = new Xhr()
  xhr.open()
  xhr.dispatchEvent(new Event('load'))
  xhr.responseType = 'json'
  xhr.response = JSON.parse(body)
  xhr.dispatchEvent(new Event('load'))
  expect(events).toHaveLength(3)
  locationStub.href = 'https://www.youtube.com/watch?v=other_video'
  xhr.dispatchEvent(new Event('load'))
  expect(events).toHaveLength(3)

  const direct = new Response('video', { headers: { 'content-type': 'video/mp4' } })
  Object.defineProperty(direct, 'url', { value: 'https://cdn.test/video.mp4' })
  fetchMock.mockResolvedValueOnce(direct)
  expect(await windowStub.fetch(direct.url)).toBe(direct)
  expect(events.at(-1)).toMatchObject({ url: direct.url, sourcePageUrl: locationStub.href })
  const count = events.length
  // A hash change keeps the same logical page and can replay its observation.
  locationStub.href += '#comments'
  windowStub.dispatchEvent(new Event('__hls_downloader_replay__'))
  expect(events).toHaveLength(count + 1)
  expect(events.at(-1).url).toBe(direct.url)
  locationStub.href = 'https://www.youtube.com/watch?v=next_video'
  windowStub.dispatchEvent(new Event('__hls_downloader_replay__'))
  expect(events).toHaveLength(count + 1)

  // An ordinary media request started on the previous page is stale too.
  let finishFetch!: (value: Response) => void
  fetchMock.mockImplementationOnce(() => new Promise(resolve => { finishFetch = resolve }))
  const pendingFetch = windowStub.fetch(direct.url)
  locationStub.href = 'https://www.youtube.com/watch?v=last_video'
  finishFetch(direct)
  expect(await pendingFetch).toBe(direct)
  expect(events).toHaveLength(count + 1)

  xhr.responseURL = direct.url
  xhr.mimeType = 'video/mp4'
  xhr.responseType = ''
  xhr.responseText = 'video'
  xhr.open()
  locationStub.href = 'https://www.youtube.com/watch?v=after_xhr'
  xhr.dispatchEvent(new Event('load'))
  expect(events).toHaveLength(count + 1)

  // The same guard applies after a cloned manifest finishes reading.
  let manifestStream!: ReadableStreamDefaultController<Uint8Array>
  const streaming = new Response(new ReadableStream<Uint8Array>({ start(controller) { manifestStream = controller } }),
    { headers: { 'content-type': 'application/vnd.apple.mpegurl' } })
  Object.defineProperty(streaming, 'url', { value: 'https://cdn.test/master.m3u8' })
  const streamClone = streaming.clone.bind(streaming)
  let finishInspection!: () => void
  const inspected = new Promise<void>(resolve => { finishInspection = resolve })
  Object.defineProperty(streaming, 'clone', { value: () => {
    const copy = streamClone()
    Object.defineProperty(copy, 'url', { value: streaming.url })
    const getReader = copy.body!.getReader.bind(copy.body!)
    Object.defineProperty(copy.body, 'getReader', { value: () => {
      const reader = getReader()
      const cancel = reader.cancel.bind(reader)
      reader.cancel = reason => {
        queueMicrotask(finishInspection)
        return cancel(reason)
      }
      return reader
    } })
    return copy
  } })
  fetchMock.mockResolvedValueOnce(streaming)
  await windowStub.fetch(streaming.url)
  expect(events).toHaveLength(count + 2)
  locationStub.href = 'https://www.youtube.com/watch?v=after_stream'
  manifestStream.enqueue(new TextEncoder().encode('#EXTM3U\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n'))
  manifestStream.close()
  await streaming.text()
  await inspected
  expect(events).toHaveLength(count + 2)
})

it.each(['arrayBuffer', 'blob', 'xhr-arraybuffer', 'xhr-blob'])('recognizes an opaque manifest read as %s without changing the returned body', async mode => {
  const url = 'https://cdn.test/get?id=42'
  const text = '#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nsegment.ts\n#EXT-X-ENDLIST\n'
  const response = new Response(text, { headers: { 'content-type': 'application/octet-stream', etag: '"media-42"' } })
  Object.defineProperty(response, 'url', { value: url })
  const windowStub = new EventTarget() as EventTarget & { fetch: typeof fetch }
  windowStub.fetch = vi.fn(async () => response)
  const bytes = new TextEncoder().encode(text).buffer
  const body = mode.endsWith('blob') ? new Blob([bytes]) : bytes
  class Xhr extends EventTarget {
    responseURL = url
    responseType = mode.endsWith('blob') ? 'blob' : 'arraybuffer'
    status = 200
    response = body
    open() {}
    getResponseHeader(name: string) { return response.headers.get(name) }
  }
  vi.stubGlobal('defineContentScript', (config: unknown) => config)
  vi.stubGlobal('window', windowStub)
  vi.stubGlobal('location', { href: 'https://site.test/watch' })
  vi.stubGlobal('XMLHttpRequest', Xhr)
  vi.stubGlobal('history', { pushState() {}, replaceState() {} })
  vi.stubGlobal('Element', class { attachShadow() {} })
  vi.stubGlobal('CustomEvent', class extends Event {
    detail: unknown
    constructor(type: string, init: { detail: unknown }) { super(type); this.detail = init.detail }
  })
  const reported = new Promise<any>(resolve => windowStub.addEventListener('__hls_downloader_resource__', event => {
    const detail = JSON.parse((event as CustomEvent).detail)
    if (detail.inspection?.inspected) resolve(detail)
  }))
  const hook = await import('../entrypoints/hooks.content')
  ;(hook.default as any).main()
  if (mode.startsWith('xhr-')) {
    const xhr = new Xhr()
    xhr.open()
    xhr.dispatchEvent(new Event('load'))
    expect(xhr.response).toBe(body)
  } else {
    const original = await windowStub.fetch(url)
    expect(original).toBe(response)
    expect(original.bodyUsed).toBe(false)
    const result = mode === 'blob' ? await original.blob() : await original.arrayBuffer()
    expect(result instanceof Blob ? await result.text() : new TextDecoder().decode(result)).toBe(text)
  }
  expect(await reported).toMatchObject({ url, mimeType: 'application/vnd.apple.mpegurl',
    inspection: { inspected: true, duration: 2, isLive: false },
    responseIdentity: { 'content-type': 'application/octet-stream', etag: '"media-42"' },
  })
})

it.each(['json', 'text'] as const)('recognizes site metadata read through %s when a player retained the original fetch', async method => {
  const url = 'https://www.youtube.com/youtubei/v1/player'
  const manifest = 'https://manifest.googlevideo.com/api/manifest/hls_variant/id/video_1'
  const value = { videoDetails: { videoId: 'video_1' }, streamingData: { hlsManifestUrl: manifest } }
  const response = new Response(JSON.stringify(value), { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(response, 'url', { value: url })
  const retainedFetch = vi.fn(async () => response)
  const windowStub = new EventTarget() as EventTarget & { fetch: typeof fetch }
  windowStub.fetch = retainedFetch
  vi.stubGlobal('defineContentScript', (config: unknown) => config)
  vi.stubGlobal('window', windowStub)
  vi.stubGlobal('location', { href: 'https://www.youtube.com/watch?v=video_1' })
  vi.stubGlobal('XMLHttpRequest', class extends EventTarget { open() {} })
  vi.stubGlobal('history', { pushState() {}, replaceState() {} })
  vi.stubGlobal('Element', class { attachShadow() {} })
  vi.stubGlobal('CustomEvent', class extends Event {
    detail: unknown
    constructor(type: string, init: { detail: unknown }) { super(type); this.detail = init.detail }
  })
  const events: any[] = []
  windowStub.addEventListener('__hls_downloader_resource__', event => {
    events.push(JSON.parse((event as CustomEvent).detail))
  })
  const hook = await import('../entrypoints/hooks.content')
  ;(hook.default as any).main()
  const fetched = await retainedFetch()
  const copy = fetched.clone()
  Object.defineProperty(copy, 'url', { value: url })
  expect(response.bodyUsed).toBe(false)
  const body = await fetched[method]()
  expect(method === 'json' ? body : JSON.parse(body)).toEqual(value)
  expect(events).toEqual([{ url: manifest, mimeType: 'application/vnd.apple.mpegurl',
    siteVideoId: 'video_1', sourcePageUrl: location.href }])
  // A clone is the same HTTP observation, not another player request.
  await copy[method]()
  expect(events).toHaveLength(1)
  expect(retainedFetch).toHaveBeenCalledOnce()

  // A fresh response at the same URL can carry a new signed manifest.
  const nextManifest = `${manifest}?signature=next`
  const nextValue = { ...value, streamingData: { hlsManifestUrl: nextManifest } }
  const repeated = new Response(JSON.stringify(nextValue), { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(repeated, 'url', { value: url })
  retainedFetch.mockResolvedValueOnce(repeated)
  await (await retainedFetch())[method]()
  expect(events).toHaveLength(2)
  expect(events.at(-1).url).toBe(nextManifest)

  const rejected = new Response(JSON.stringify(value), { status: 403, headers: { 'content-type': 'application/json' } })
  Object.defineProperty(rejected, 'url', { value: url })
  expect(await rejected.json()).toEqual(value)
  const ordinary = new Response(JSON.stringify(value), { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(ordinary, 'url', { value: 'https://site.test/analytics' })
  expect(await ordinary.json()).toEqual(value)
  const invalid = new Response('not JSON', { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(invalid, 'url', { value: url })
  await expect(invalid.json()).rejects.toThrow(SyntaxError)
  expect(events).toHaveLength(2)

  let stream!: ReadableStreamDefaultController<Uint8Array>
  const delayed = new Response(new ReadableStream<Uint8Array>({ start(controller) { stream = controller } }),
    { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(delayed, 'url', { value: url })
  retainedFetch.mockResolvedValueOnce(delayed)
  const tracked = await windowStub.fetch(url)
  const pendingBody = tracked[method]()
  // Keep the video ID the same so page ownership, not the site's ID check,
  // must reject the old response after a real navigation.
  location.href += '&list=other'
  stream.enqueue(new TextEncoder().encode(JSON.stringify(value)))
  stream.close()
  const delayedBody = await pendingBody
  expect(method === 'json' ? delayedBody : JSON.parse(delayedBody)).toEqual(value)
  expect(events).toHaveLength(2)
})

it.each(['fetch', 'xhr'])('keeps rejected media responses out of the %s bridge', async transport => {
  const url = 'https://cdn.test/rejected.m3u8'
  const windowStub = new EventTarget() as EventTarget & { fetch: typeof fetch }
  let response: Response
  windowStub.fetch = vi.fn(async () => response)
  class Xhr extends EventTarget {
    responseURL = url
    status = 403
    responseType = 'text'
    responseText = '#EXTM3U\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n'
    open() {}
    getResponseHeader() { return 'application/vnd.apple.mpegurl' }
  }
  vi.stubGlobal('defineContentScript', (config: unknown) => config)
  vi.stubGlobal('window', windowStub)
  vi.stubGlobal('location', { href: 'https://site.test/watch' })
  vi.stubGlobal('XMLHttpRequest', Xhr)
  vi.stubGlobal('history', { pushState() {}, replaceState() {} })
  vi.stubGlobal('Element', class { attachShadow() {} })
  vi.stubGlobal('CustomEvent', class extends Event {
    detail: unknown
    constructor(type: string, init: { detail: unknown }) { super(type); this.detail = init.detail }
  })
  const events: any[] = []
  windowStub.addEventListener('__hls_downloader_resource__', event => events.push((event as CustomEvent).detail))
  const hook = await import('../entrypoints/hooks.content')
  ;(hook.default as any).main()
  for (const status of [204, 205, 302, 403, 503]) {
    if (transport === 'fetch') {
      response = new Response([204, 205].includes(status) ? null : 'error', {
        status, headers: { 'content-type': 'application/vnd.apple.mpegurl' },
      })
      Object.defineProperty(response, 'url', { value: url })
      expect(await windowStub.fetch(url)).toBe(response)
    } else {
      const xhr = new Xhr()
      xhr.status = status
      xhr.open()
      xhr.dispatchEvent(new Event('load'))
    }
  }
  expect(events).toEqual([])
})

it.each(['pushState', 'replaceState', 'popstate', 'pagehide', 'fragment'] as const)(
  'releases a pending cloned inspection on %s without cancelling the player response', async navigation => {
    const text = '#EXTM3U\n#EXTINF:2,\nsegment.ts\n#EXT-X-ENDLIST\n'
    const readBody = Response.prototype.text
    let source!: ReadableStreamDefaultController<Uint8Array>
    const response = new Response(new ReadableStream<Uint8Array>({ start(controller) { source = controller } }),
      { headers: { 'content-type': 'application/vnd.apple.mpegurl' } })
    source.enqueue(new TextEncoder().encode(text.slice(0, 10)))
    Object.defineProperty(response, 'url', { value: 'https://cdn.test/master.m3u8?token=original' })
    const clone = response.clone.bind(response)
    const cancelled = vi.fn()
    let started!: () => void
    const reading = new Promise<void>(resolve => { started = resolve })
    Object.defineProperty(response, 'clone', { value: () => {
      const copy = clone()
      Object.defineProperty(copy, 'url', { value: response.url })
      const getReader = copy.body!.getReader.bind(copy.body!)
      Object.defineProperty(copy.body, 'getReader', { value: () => {
        const reader = getReader()
        const read = reader.read.bind(reader)
        const cancel = reader.cancel.bind(reader)
        let reads = 0
        reader.read = () => { if (++reads === 2) started(); return read() }
        reader.cancel = reason => { cancelled(); return cancel(reason) }
        return reader
      } })
      return copy
    } })
    const windowStub = new EventTarget() as EventTarget & { fetch: typeof fetch }
    const fetchMock = vi.fn(async () => response)
    windowStub.fetch = fetchMock
    const locationStub = { href: 'https://site.test/watch?v=original' }
    const historyStub = {
      pushState(_state: unknown, _title: string, url?: string | URL | null) { locationStub.href = String(url) },
      replaceState(_state: unknown, _title: string, url?: string | URL | null) { locationStub.href = String(url) },
    }
    vi.stubGlobal('defineContentScript', (config: unknown) => config)
    vi.stubGlobal('window', windowStub)
    vi.stubGlobal('location', locationStub)
    vi.stubGlobal('history', historyStub)
    vi.stubGlobal('XMLHttpRequest', class extends EventTarget { open() {} })
    vi.stubGlobal('Element', class { attachShadow() {} })
    vi.stubGlobal('CustomEvent', class extends Event {
      detail: unknown
      constructor(type: string, init: { detail: unknown }) { super(type); this.detail = init.detail }
    })
    const events: any[] = []
    let inspected!: () => void
    const inspection = new Promise<void>(resolve => { inspected = resolve })
    windowStub.addEventListener('__hls_downloader_resource__', event => {
      const detail = JSON.parse((event as CustomEvent).detail)
      events.push(detail)
      if (detail.inspection?.inspected) inspected()
    })
    const hook = await import('../entrypoints/hooks.content')
    ;(hook.default as any).main()
    try {
      expect(await windowStub.fetch(response.url)).toBe(response)
      await reading
      expect(response.bodyUsed).toBe(false)
      if (navigation === 'pushState' || navigation === 'replaceState') {
        historyStub[navigation](null, '', `${locationStub.href}&list=other`)
      } else if (navigation === 'fragment') {
        historyStub.pushState(null, '', `${locationStub.href}#comments`)
        windowStub.dispatchEvent(new Event('hashchange'))
      } else {
        if (navigation === 'popstate') locationStub.href = 'https://site.test/other'
        windowStub.dispatchEvent(new Event(navigation))
      }
      // Flush the History navigation signal without waiting for another network byte.
      await new Promise<void>(resolve => queueMicrotask(resolve))
      expect(cancelled).toHaveBeenCalledTimes(navigation === 'fragment' ? 0 : 1)
    } finally {
      source.enqueue(new TextEncoder().encode(text.slice(10)))
      source.close()
      expect(await readBody.call(response)).toBe(text)
    }
    if (navigation === 'fragment') await inspection
    expect(fetchMock).toHaveBeenCalledOnce()
    expect(events.filter(event => event.inspection?.inspected)).toHaveLength(navigation === 'fragment' ? 1 : 0)
  },
)

it('extracts Bilibili __playinfo__ and playurl JSON on /en/video/ pages without blocking responses', async () => {
  const pageUrl = 'https://www.bilibili.com/en/video/BV1xx411c7mD/'
  const playurl = 'https://api.bilibili.com/x/player/wbi/playurl?bvid=BV1xx411c7mD&cid=123'
  const video1080 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30080.m4s?deadline=1900000000'
  const video720 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30064.m4s?deadline=1900000000'
  const audio192 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30280.m4s?deadline=1900000000'
  const playinfo = {
    code: 0,
    data: {
      dash: {
        duration: 185,
        video: [
          { id: 80, baseUrl: video1080, width: 1920, height: 1080, bandwidth: 3_200_000, codecs: 'avc1.640032' },
          { id: 64, baseUrl: video720, width: 1280, height: 720, bandwidth: 1_500_000, codecs: 'avc1.64001f' },
        ],
        audio: [{ id: 30280, baseUrl: audio192, bandwidth: 192_000, codecs: 'mp4a.40.2' }],
      },
    },
  }
  const response = new Response(JSON.stringify(playinfo), { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(response, 'url', { value: playurl })
  const clone = response.clone.bind(response)
  Object.defineProperty(response, 'clone', {
    value: () => {
      const copy = clone()
      Object.defineProperty(copy, 'url', { value: playurl })
      return copy
    },
  })
  const windowStub = new EventTarget() as EventTarget & { fetch: typeof fetch; __playinfo__?: unknown }
  const fetchMock = vi.fn(async () => response)
  windowStub.fetch = fetchMock
  windowStub.__playinfo__ = playinfo
  const locationStub = { href: pageUrl }
  const historyStub = {
    pushState(_state: unknown, _title: string, url?: string | URL | null) { if (url) locationStub.href = String(url) },
    replaceState(_state: unknown, _title: string, url?: string | URL | null) { if (url) locationStub.href = String(url) },
  }
  vi.stubGlobal('defineContentScript', (config: unknown) => config)
  vi.stubGlobal('window', windowStub)
  vi.stubGlobal('location', locationStub)
  vi.stubGlobal('XMLHttpRequest', class extends EventTarget { open() {} })
  vi.stubGlobal('history', historyStub)
  vi.stubGlobal('Element', class { attachShadow() {} })
  vi.stubGlobal('CustomEvent', class extends Event {
    detail: unknown
    constructor(type: string, init: { detail: unknown }) { super(type); this.detail = init.detail }
  })
  const events: any[] = []
  windowStub.addEventListener('__hls_downloader_resource__', event => {
    events.push(JSON.parse((event as CustomEvent).detail))
  })
  const hook = await import('../entrypoints/hooks.content')
  ;(hook.default as any).main()

  // Replay emits SSR window.__playinfo__ immediately with audioUrl.
  windowStub.dispatchEvent(new Event('__hls_downloader_replay__'))
  expect(events.some(event => event.url === video1080 && event.audioUrl === audio192 && event.siteVideoId === 'BV1xx411c7mD' && event.inspection?.inspected === true)).toBe(true)

  // Fetch of Bilibili playurl API also reports without consuming the original response.
  const fetched = await windowStub.fetch(playurl)
  expect(fetched.bodyUsed).toBe(false)
  expect(await fetched.json()).toEqual(playinfo)
  expect(events.at(-1)).toMatchObject({
    url: video1080,
    audioUrl: audio192,
    mimeType: 'video/mp4',
    siteVideoId: 'BV1xx411c7mD',
    sourcePageUrl: pageUrl,
    inspection: {
      inspected: true,
      audioUrl: audio192,
      filename: 'BV1xx411c7mD.mp4',
      duration: 185,
      height: 1080,
      quality: '1080p',
    },
  })

  // SPA replaceState normalization (/en/video/BV... -> /video/BV...?vd_source=1) preserves buffered candidates on replay.
  const redirectedUrl = 'https://www.bilibili.com/video/BV1xx411c7mD/?vd_source=1'
  delete windowStub.__playinfo__
  historyStub.replaceState(null, '', redirectedUrl)
  await new Promise<void>(resolve => queueMicrotask(resolve))
  events.length = 0
  windowStub.dispatchEvent(new Event('__hls_downloader_replay__'))
  expect(events.some(event => event.url === video1080 && event.audioUrl === audio192 && event.sourcePageUrl === redirectedUrl)).toBe(true)

  // SPA pushState to a new BV video ignores stale SSR window.__playinfo__ from Video A while capturing Video B's playurl fetch.
  windowStub.__playinfo__ = playinfo
  const nextVideoPage = 'https://www.bilibili.com/video/BV1yy411c7mE/'
  const nextPlayurl = 'https://api.bilibili.com/x/player/wbi/playurl?bvid=BV1yy411c7mE&cid=456'
  const nextVideo1080 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/9/9/456-1-30080.m4s?deadline=1900000000'
  const nextAudio192 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/9/9/456-1-30280.m4s?deadline=1900000000'
  events.length = 0
  historyStub.pushState(null, '', nextVideoPage)
  await new Promise<void>(resolve => queueMicrotask(resolve))
  windowStub.dispatchEvent(new Event('__hls_downloader_replay__'))
  expect(events).toEqual([])

  const nextPayload = {
    code: 0,
    data: {
      dash: {
        duration: 95,
        video: [{ id: 80, baseUrl: nextVideo1080, width: 1920, height: 1080, bandwidth: 3_100_000, codecs: 'avc1.640032' }],
        audio: [{ id: 30280, baseUrl: nextAudio192, bandwidth: 192_000, codecs: 'mp4a.40.2' }],
      },
    },
  }
  const nextResponse = new Response(JSON.stringify(nextPayload), { headers: { 'content-type': 'application/json' } })
  Object.defineProperty(nextResponse, 'url', { value: nextPlayurl })
  const cloneNext = nextResponse.clone.bind(nextResponse)
  Object.defineProperty(nextResponse, 'clone', {
    value: () => {
      const copy = cloneNext()
      Object.defineProperty(copy, 'url', { value: nextPlayurl })
      return copy
    },
  })
  fetchMock.mockResolvedValueOnce(nextResponse)
  const fetchedNext = await windowStub.fetch(nextPlayurl)
  expect(await fetchedNext.json()).toEqual(nextPayload)
  expect(events).toHaveLength(1)
  expect(events[0]).toMatchObject({
    url: nextVideo1080,
    audioUrl: nextAudio192,
    siteVideoId: 'BV1yy411c7mE',
    sourcePageUrl: nextVideoPage,
  })
})
