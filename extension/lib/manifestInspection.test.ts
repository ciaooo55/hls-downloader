import { describe, expect, it, vi } from 'vitest'
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'
import { createManifestFetcher, type ManifestFetcher } from './manifestInspection'

describe('manifest request policy', () => {
  it('stops an obsolete inspection before collecting request credentials', async () => {
    const controller = new AbortController()
    controller.abort()
    const requestContextForUrl = vi.fn(async () => ({ requestHeaders: { Authorization: 'Bearer old-page' } }))
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('unexpected'))
    await expect(createManifestFetcher({
      url: 'https://cdn.test/manifest', signal: controller.signal, requestContextForUrl,
    }, fetcher)('https://cdn.test/manifest')).rejects.toMatchObject({ name: 'AbortError' })
    expect(requestContextForUrl).not.toHaveBeenCalled()
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('does not issue a request if its page changes during credential collection', async () => {
    const controller = new AbortController()
    let release!: (value: {}) => void
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('unexpected'))
    const request = createManifestFetcher({
      url: 'https://cdn.test/manifest', signal: controller.signal,
      requestContextForUrl: () => new Promise(resolve => { release = resolve }),
    }, fetcher)('https://cdn.test/manifest')
    controller.abort()
    release({})
    await expect(request).rejects.toMatchObject({ name: 'AbortError' })
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('does not retry with fallback credentials after its page is aborted', async () => {
    const controller = new AbortController()
    const fetcher = vi.fn<ManifestFetcher>(async () => {
      controller.abort()
      throw controller.signal.reason
    })
    await expect(createManifestFetcher({
      url: 'https://cdn.test/manifest', signal: controller.signal,
      inspectionHeaders: { Cookie: 'session=old-page' },
    }, fetcher)('https://cdn.test/manifest')).rejects.toMatchObject({ name: 'AbortError' })
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(fetcher.mock.calls[0][1].signal?.aborted).toBe(true)
  })

  it('bounds contextual headers while preserving replay headers', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('ok'))
    await createManifestFetcher({
      url: 'https://cdn.test/manifest',
      requestHeaders: { Authorization: 'Bearer media', 'User-Agent': 'captured', 'Sec-Fetch-Site': 'same-origin' },
      inspectionHeaders: {
        Referer: ' https://page.test/watch ',
        Cookie: 'session=media',
        Origin: 'https://page.test\r\nx-injected: yes',
        Authorization: 'untrusted-override',
        'X-Other': 'ignored',
      },
    }, fetcher)('https://cdn.test/manifest')
    expect(fetcher.mock.calls[0][1].headers).toEqual({
      authorization: 'Bearer media', referer: 'https://page.test/watch', cookie: 'session=media',
    })
  })

  it('does not retry a failed fallback again after a network failure', async () => {
    const fallbackFailure = new Error('fallback unavailable')
    const fetcher = vi.fn<ManifestFetcher>()
    fetcher.mockRejectedValueOnce(new Error('context blocked'))
    fetcher.mockRejectedValueOnce(fallbackFailure)

    await expect(createManifestFetcher({ url: 'https://cdn.test/manifest', inspectionHeaders: { Cookie: 'session=media' } }, fetcher)(
      'https://cdn.test/manifest',
    )).rejects.toBe(fallbackFailure)
    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls[1][1].headers).toEqual({})
    expect(fetcher.mock.calls[1][1].signal).toBe(fetcher.mock.calls[0][1].signal)
  })

  it('preserves an HTTP rejection without making a second authenticated request', async () => {
    let cancelled = false
    const rejected = new Response(new ReadableStream({ cancel() { cancelled = true } }), { status: 403 })
    const fetcher = vi.fn<ManifestFetcher>().mockResolvedValue(rejected)
    const response = await createManifestFetcher({ url: 'https://cdn.test/manifest', inspectionHeaders: { Origin: 'https://page.test' } }, fetcher)(
      'https://cdn.test/manifest',
    )
    expect(response).toBe(rejected)
    expect(cancelled).toBe(true)
    expect(fetcher).toHaveBeenCalledTimes(1)
  })

  it('does not repeat a request without contextual headers', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('unavailable', { status: 503 }))
    const response = await createManifestFetcher({ url: 'https://cdn.test/manifest' }, fetcher)('https://cdn.test/manifest')
    expect(response.status).toBe(503)
    expect(fetcher).toHaveBeenCalledTimes(1)
  })

  it('keeps a master cookie on its exact URL and excludes its tokens from another CDN', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('ok'))
    const inspect = createManifestFetcher({
      url: 'https://master.test/main.m3u8',
      requestHeaders: { Authorization: 'Bearer master', 'X-Playback-Token': 'private', Accept: '*/*' },
      inspectionHeaders: { Cookie: 'sid=master', Referer: 'https://page.test/watch' },
    }, fetcher)
    await inspect('https://master.test/main.m3u8')
    await inspect('https://master.test/other/child.m3u8')
    await inspect('https://video.test/child.m3u8')
    expect(fetcher.mock.calls[0][1].headers).toMatchObject({ cookie: 'sid=master', authorization: 'Bearer master' })
    expect(fetcher.mock.calls[1][1].headers).toMatchObject({ authorization: 'Bearer master' })
    expect(fetcher.mock.calls[1][1].headers).not.toHaveProperty('cookie')
    expect(fetcher.mock.calls[2][1].headers).toEqual({ accept: '*/*', referer: 'https://page.test/watch' })
    expect(fetcher.mock.calls[0][1].redirect).toBe('error')
  })

  it('selects the target identity again for each rendition and its contextual fallback', async () => {
    const fetcher = vi.fn<ManifestFetcher>()
      .mockResolvedValueOnce(new Response('master'))
      .mockRejectedValueOnce(new TypeError('context blocked'))
      .mockResolvedValueOnce(new Response('child'))
    const inspect = createManifestFetcher({
      url: 'https://master.test/main.m3u8',
      requestHeaders: { Authorization: 'Bearer wrong' },
      requestContextForUrl: async url => ({
        requestHeaders: { Authorization: `Bearer ${new URL(url).hostname}` },
        inspectionHeaders: { Cookie: `sid=${new URL(url).hostname}` },
      }),
    }, fetcher)
    await inspect('https://master.test/main.m3u8')
    await inspect('https://video.test/child.m3u8')
    expect(fetcher.mock.calls.map(([, init]) => init.headers)).toEqual([
      { authorization: 'Bearer master.test', cookie: 'sid=master.test' },
      { authorization: 'Bearer video.test', cookie: 'sid=video.test' },
      { authorization: 'Bearer video.test' },
    ])
  })

  it('does not forward a captured token through an unseen HTTP redirect', async () => {
    const paths: string[] = []
    const server = createServer((request, response) => {
      paths.push(request.url || '')
      response.writeHead(request.url === '/manifest' ? 302 : 200, { Location: '/unobserved' })
      response.end()
    })
    await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
    try {
      const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/manifest`
      await expect(createManifestFetcher({ url, requestHeaders: { 'X-Playback-Token': 'private' } })(url)).rejects.toThrow()
      expect(paths).toEqual(['/manifest'])
    } finally {
      await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()))
    }
  })

  it('aborts an actual HTTP response body when the owning page disappears', async () => {
    const controller = new AbortController()
    const server = createServer((_request, response) => {
      response.writeHead(200, { 'Content-Type': 'application/vnd.apple.mpegurl' })
      response.write('#EXTM3U\n')
    })
    await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
    try {
      const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/manifest`
      const response = await createManifestFetcher({ url, signal: controller.signal })(url)
      const reading = response.text()
      controller.abort()
      await expect(reading).rejects.toMatchObject({ name: 'AbortError' })
    } finally {
      controller.abort()
      server.closeAllConnections()
      await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()))
    }
  })
})
