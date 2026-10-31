import { describe, expect, it, vi } from 'vitest'
import { createManifestFetcher, type ManifestFetcher } from './manifestInspection'

describe('manifest request policy', () => {
  it('bounds contextual headers while preserving replay headers', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('ok'))
    await createManifestFetcher({
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

  it.each(['response', 'network'] as const)('does not retry a failed fallback again after a %s failure', async failure => {
    const fallbackFailure = new Error('fallback unavailable')
    const fetcher = vi.fn<ManifestFetcher>()
    if (failure === 'response') fetcher.mockResolvedValueOnce(new Response('forbidden', { status: 403 }))
    else fetcher.mockRejectedValueOnce(new Error('context blocked'))
    fetcher.mockRejectedValueOnce(fallbackFailure)

    await expect(createManifestFetcher({ inspectionHeaders: { Cookie: 'session=media' } }, fetcher)(
      'https://cdn.test/manifest',
    )).rejects.toBe(fallbackFailure)
    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls[1][1].headers).toEqual({})
    expect(fetcher.mock.calls[1][1].signal).toBe(fetcher.mock.calls[0][1].signal)
  })

  it('cancels a rejected response before opening its replacement', async () => {
    let cancelled = false
    const rejected = new Response(new ReadableStream({ cancel() { cancelled = true } }), { status: 403 })
    const fetcher = vi.fn<ManifestFetcher>()
      .mockResolvedValueOnce(rejected)
      .mockImplementationOnce(async () => {
        expect(cancelled).toBe(true)
        return new Response('ok')
      })
    const response = await createManifestFetcher({ inspectionHeaders: { Origin: 'https://page.test' } }, fetcher)(
      'https://cdn.test/manifest',
    )
    expect(response.ok).toBe(true)
  })

  it('does not repeat a request without contextual headers', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('unavailable', { status: 503 }))
    const response = await createManifestFetcher({}, fetcher)('https://cdn.test/manifest')
    expect(response.status).toBe(503)
    expect(fetcher).toHaveBeenCalledTimes(1)
  })
})
