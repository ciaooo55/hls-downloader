import { describe, expect, it } from 'vitest'
import { captureReplayableRequestBody, replayablePostRequest, RequestChainStore, requestHeader, responseHeader } from './requestChain'

describe('browser request chains', () => {
  it('keeps the initial PHP URL and the final redirected file together', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'one', url: 'https://site.test/download.php?id=1', tabId: 7,
      frameId: 0, type: 'main_frame', method: 'GET', documentUrl: 'https://site.test/page',
      requestHeaders: [{ name: 'Referer', value: 'https://site.test/page' }], timeStamp: 1000,
    })
    store.observeRedirect({
      requestId: 'one', url: 'https://site.test/download.php?id=1', redirectUrl: 'https://cdn.test/setup.exe',
      tabId: 7, statusCode: 302, timeStamp: 1100,
    })
    store.observeResponse({
      requestId: 'one', url: 'https://cdn.test/setup.exe', tabId: 7, statusCode: 200, timeStamp: 1200,
      responseHeaders: [
        { name: 'Content-Disposition', value: 'attachment; filename="setup.exe"' },
        { name: 'Content-Length', value: '2048' },
      ],
    })

    const chain = store.find({ url: 'https://site.test/download.php?id=1' }, 1300)
    expect(chain?.initialUrl).toBe('https://site.test/download.php?id=1')
    expect(chain?.finalUrl).toBe('https://cdn.test/setup.exe')
    expect(chain?.urls).toEqual([
      'https://site.test/download.php?id=1',
      'https://cdn.test/setup.exe',
    ])
    expect(requestHeader(chain, 'referer')).toBe('https://site.test/page')
    expect(responseHeader(chain, 'content-length')).toBe('2048')
  })

  it('does not mix concurrent requests with different request ids', () => {
    const store = new RequestChainStore()
    store.observeRequest({ requestId: 'a', url: 'https://a.test/file', tabId: 1, timeStamp: 1000 })
    store.observeRequest({ requestId: 'b', url: 'https://b.test/file', tabId: 2, timeStamp: 1001 })
    expect(store.find({ url: 'https://b.test/file' }, 1100)?.requestId).toBe('b')
  })

  it('finds the exact headers for a canonicalized LL-HLS playlist', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'live-poll',
      url: 'https://edge.test/live.m3u8?session=current&_HLS_msn=50&_HLS_part=2',
      tabId: 4,
      timeStamp: 1000,
      requestHeaders: [{ name: 'X-Playback-Token', value: 'captured' }],
    })

    const chain = store.find({ url: 'https://edge.test/live.m3u8?session=current' }, 1100, 4)
    expect(chain?.requestId).toBe('live-poll')
    expect(requestHeader(chain, 'x-playback-token')).toBe('captured')
  })

  it('uses the newest successful signed LL-HLS poll for an older panel item', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'old-token', url: 'https://edge.test/live.m3u8?token=old&_HLS_msn=4',
      tabId: 9, type: 'xmlhttprequest', timeStamp: 1000,
    })
    store.observeResponse({
      requestId: 'old-token', url: 'https://edge.test/live.m3u8?token=old&_HLS_msn=4',
      tabId: 9, type: 'xmlhttprequest', timeStamp: 1010, statusCode: 200,
    })
    store.observeRequest({
      requestId: 'fresh-token', url: 'https://edge.test/live.m3u8?token=fresh&_HLS_msn=8&_HLS_part=2',
      tabId: 9, type: 'xmlhttprequest', timeStamp: 1200,
      requestHeaders: [{ name: 'Referer', value: 'https://page.test/watch' }],
    })
    store.observeResponse({
      requestId: 'fresh-token', url: 'https://edge.test/live.m3u8?token=fresh&_HLS_msn=8&_HLS_part=2',
      tabId: 9, type: 'xmlhttprequest', timeStamp: 1210, statusCode: 200,
    })
    store.observeResponse({
      requestId: 'rejected-token', url: 'https://edge.test/live.m3u8?token=rejected&_HLS_msn=9',
      tabId: 9, type: 'xmlhttprequest', timeStamp: 1250, statusCode: 403,
    })

    const chain = store.find({ url: 'https://edge.test/live.m3u8?token=old' }, 1300, 9, true)
    expect(chain?.requestId).toBe('fresh-token')
    expect(chain?.finalUrl).toContain('token=fresh')
    expect(requestHeader(chain, 'referer')).toBe('https://page.test/watch')
  })

  it('matches a signed MP4 observation to the latest successful browser request', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'signed-mp4',
      url: 'https://gfve4dog1.mxcontent.net/v2/asset.mp4?s=fresh&e=200&_t=150&quality=1080',
      tabId: 3,
      type: 'media',
      timeStamp: 1000,
      requestHeaders: [{ name: 'Referer', value: 'https://page.test/watch' }],
    })
    store.observeResponse({
      requestId: 'signed-mp4',
      url: 'https://gfve4dog1.mxcontent.net/v2/asset.mp4?s=fresh&e=200&_t=150&quality=1080',
      tabId: 3,
      type: 'media',
      statusCode: 206,
      timeStamp: 1100,
    })

    const matched = store.find({
      url: 'https://gfve4dog1.mxcontent.net/v2/asset.mp4?s=stale&e=100&_t=90&quality=1080',
    }, 1200, 3, true)

    expect(matched?.finalUrl).toContain('s=fresh')
    expect(matched?.statusCode).toBe(206)
  })

  it('does not merge signed files when a meaningful selector differs', () => {
    const store = new RequestChainStore()
    store.observeResponse({
      requestId: 'quality-720',
      url: 'https://cdn.test/movie.mp4?s=fresh&e=200&_t=150&quality=720',
      tabId: 3,
      statusCode: 200,
      timeStamp: 1000,
    })

    expect(store.find({
      url: 'https://cdn.test/movie.mp4?s=old&e=100&_t=90&quality=1080',
    }, 1100, 3, true)).toBeUndefined()
  })

  it('bounds request history while retaining a recent failed request', () => {
    const store = new RequestChainStore(3)
    for (let index = 1; index <= 4; index += 1) {
      store.observeRequest({
        requestId: `request-${index}`,
        url: `https://cdn.test/${index}.bin`,
        tabId: 1,
        timeStamp: 1000 + index,
      })
    }

    expect(store.find({ url: 'https://cdn.test/1.bin' }, 1100)).toBeUndefined()
    expect(store.find({ url: 'https://cdn.test/4.bin' }, 1100)?.requestId).toBe('request-4')
    store.fail('request-4', 1_200)
    expect(store.find({ url: 'https://cdn.test/4.bin' }, 1_300)?.requestId).toBe('request-4')
  })

  it('prefers the request from the download referrer when URLs are shared across tabs', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'wanted', url: 'https://cdn.test/get.php', tabId: 1,
      documentUrl: 'https://site.test/page', timeStamp: 1000,
    })
    store.observeRequest({
      requestId: 'other', url: 'https://cdn.test/get.php', tabId: 2,
      documentUrl: 'https://ads.test/page', timeStamp: 1100,
    })

    expect(store.find({
      url: 'https://cdn.test/get.php',
      referrer: 'https://site.test/page',
    }, 1200)?.requestId).toBe('wanted')
  })

  it('uses the click-intent tab and never borrows headers from another tab', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'tab-one', url: 'https://cdn.test/shared.bin', tabId: 1, timeStamp: 1000,
      requestHeaders: [{ name: 'Authorization', value: 'Bearer one' }],
    })
    store.observeRequest({
      requestId: 'tab-two', url: 'https://cdn.test/shared.bin', tabId: 2, timeStamp: 1100,
      requestHeaders: [{ name: 'Authorization', value: 'Bearer two' }],
    })

    expect(store.find({ url: 'https://cdn.test/shared.bin' }, 1200, 1)?.requestId).toBe('tab-one')
    expect(store.find({ url: 'https://cdn.test/shared.bin' }, 1200, 3)).toBeUndefined()
  })

  it('collects only recent media contexts from the active tab and page', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'manifest', url: 'https://media.test/master.m3u8', tabId: 7,
      type: 'xmlhttprequest', documentUrl: 'https://page.test/watch', timeStamp: 1000,
      requestHeaders: [{ name: 'Authorization', value: 'Bearer media' }],
    })
    store.observeRequest({
      requestId: 'segment-old', url: 'https://cdn.test/1.ts', tabId: 7,
      type: 'media', documentUrl: 'https://page.test/watch', timeStamp: 1050,
      requestHeaders: [{ name: 'X-Playback-Token', value: 'old' }],
    })
    store.observeRequest({
      requestId: 'segment-new', url: 'https://cdn.test/2.ts', tabId: 7,
      type: 'media', documentUrl: 'https://page.test/watch', timeStamp: 1100,
      requestHeaders: [{ name: 'X-Playback-Token', value: 'new' }],
    })
    store.observeRequest({
      requestId: 'wrong-page', url: 'https://private.test/secret.ts', tabId: 7,
      type: 'media', documentUrl: 'https://page.test/other', timeStamp: 1150,
    })
    store.observeRequest({
      requestId: 'wrong-tab', url: 'https://ads.test/ad.ts', tabId: 8,
      type: 'media', documentUrl: 'https://page.test/watch', timeStamp: 1160,
    })
    store.observeRequest({
      requestId: 'script', url: 'https://static.test/app.js', tabId: 7,
      type: 'script', documentUrl: 'https://page.test/watch', timeStamp: 1170,
    })

    const contexts = store.contextsForPage(7, 'https://page.test/watch', 1200)
    expect(contexts.map(item => item.requestId)).toEqual(['segment-new', 'manifest'])
  })

  it('keeps a recent playback context long enough for a user to choose a resource', () => {
    const store = new RequestChainStore()
    store.observeRequest({ requestId: 'old', url: 'https://a.test/file', tabId: 1, timeStamp: 1000 })
    expect(store.find({ url: 'https://a.test/file' }, 240_000)?.requestId).toBe('old')
    expect(store.find({ url: 'https://a.test/file' }, 301_001)).toBeUndefined()
  })

  it('keeps the source-page navigation as the default browser context', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'page', url: 'https://site.test/watch/42', tabId: 7,
      type: 'main_frame', timeStamp: 1000,
      requestHeaders: [{ name: 'User-Agent', value: 'Browser UA' }],
    })
    store.observeRequest({
      requestId: 'other', url: 'https://site.test/watch/other', tabId: 7,
      type: 'main_frame', timeStamp: 1100,
    })

    const context = store.pageContext(7, 'https://site.test/watch/42', 1200)
    expect(context?.requestId).toBe('page')
    expect(requestHeader(context, 'user-agent')).toBe('Browser UA')
  })

  it('keeps a small JSON POST body only for a matching replayable download request', () => {
    const store = new RequestChainStore()
    const bytes = new TextEncoder().encode('{"asset":"episode-12","token":"short-lived"}')
    store.observeRequest({
      requestId: 'post-download', url: 'https://api.test/export', tabId: 3,
      method: 'POST', timeStamp: 1000,
      requestBody: { raw: [{ bytes: bytes.buffer }] },
    })
    const chain = store.observeRequest({
      requestId: 'post-download', url: 'https://api.test/export', tabId: 3,
      method: 'POST', timeStamp: 1001,
      requestHeaders: [{ name: 'Content-Type', value: 'application/json; charset=utf-8' }],
    })

    expect(atob(chain.requestBody)).toBe('{"asset":"episode-12","token":"short-lived"}')
    expect(replayablePostRequest(chain)).toEqual({ request_method: 'POST', request_body: chain.requestBody })
  })

  it('never reconstructs multipart uploads or hands them to the desktop app', () => {
    const body = captureReplayableRequestBody({ raw: [{ bytes: new Uint8Array([1, 2]).buffer }, { bytes: new Uint8Array([3]).buffer }] })
    expect(body).toBe('')

    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'upload', url: 'https://api.test/export', tabId: 3, method: 'POST', timeStamp: 1000,
      requestBody: { raw: [{ bytes: new Uint8Array([1, 2]).buffer }] },
    })
    const chain = store.observeRequest({
      requestId: 'upload', url: 'https://api.test/export', tabId: 3, method: 'POST', timeStamp: 1001,
      requestHeaders: [{ name: 'Content-Type', value: 'multipart/form-data; boundary=browser' }],
    })
    expect(replayablePostRequest(chain)).toEqual({})
  })

  it('bounds captured headers and strips header injection bytes', () => {
    const store = new RequestChainStore()
    const chain = store.observeRequest({
      requestId: 'bounded-headers',
      url: 'https://api.test/export',
      tabId: 3,
      timeStamp: 1000,
      requestHeaders: [
        { name: 'X-Test\r\nInjected', value: 'bad' },
        { name: 'Authorization', value: `Bearer ${'x'.repeat(40_000)}` },
        ...Array.from({ length: 80 }, (_, index) => ({ name: `X-${index}`, value: 'v' })),
      ],
    })

    expect(requestHeader(chain, 'x-test\r\ninjected')).toBe('')
    expect(requestHeader(chain, 'authorization').length).toBeLessThanOrEqual(16 * 1024)
    expect(Object.keys(chain.requestHeaders).length).toBeLessThanOrEqual(64)
    expect(Object.values(chain.requestHeaders).join('').length).toBeLessThanOrEqual(32 * 1024)
  })

  it('rejects oversized replay bodies before copying them', () => {
    expect(captureReplayableRequestBody({
      raw: [{ bytes: new Uint8Array(128 * 1024 + 1).buffer }],
    })).toBe('')
    expect(captureReplayableRequestBody({
      formData: { value: ['x'.repeat(300_000)] },
    })).toBe('')
  })

  it('drops only the navigated tab request chains', () => {
    const store = new RequestChainStore()
    store.observeRequest({ requestId: 'old-page', url: 'https://cdn.test/old.m3u8', tabId: 7, timeStamp: 1000 })
    store.observeRequest({ requestId: 'other-tab', url: 'https://cdn.test/other.m3u8', tabId: 8, timeStamp: 1000 })

    store.clearTab(7)

    expect(store.find({ url: 'https://cdn.test/old.m3u8' }, 1200)).toBeUndefined()
    expect(store.find({ url: 'https://cdn.test/other.m3u8' }, 1200)?.requestId).toBe('other-tab')
  })

  it('keeps a failed request briefly for the downloads.onCreated race', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'paused-download',
      url: 'https://download.test/generated-file',
      tabId: 7,
      type: 'main_frame',
      method: 'GET',
      documentUrl: 'https://page.test/export',
      requestHeaders: [{ name: 'Referer', value: 'https://page.test/export' }],
      timeStamp: 1_000,
    })
    store.fail('paused-download', 1_100)

    expect(store.find({ url: 'https://download.test/generated-file' }, 20_000)?.requestId)
      .toBe('paused-download')
    expect(store.find({ url: 'https://download.test/generated-file' }, 21_101))
      .toBeUndefined()
  })
})
