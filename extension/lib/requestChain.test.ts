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

  it('expires completed request metadata', () => {
    const store = new RequestChainStore()
    store.observeRequest({ requestId: 'old', url: 'https://a.test/file', tabId: 1, timeStamp: 1000 })
    expect(store.find({ url: 'https://a.test/file' }, 32_000)).toBeUndefined()
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
})
