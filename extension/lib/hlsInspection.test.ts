import { describe, expect, it, vi } from 'vitest'

import { inspectHlsResource, type ManifestFetcher } from './hlsInspection'

const master = '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=8000000,RESOLUTION=1920x1080\n1080/index.m3u8\n'
const vod = '#EXTM3U\n#EXTINF:6,\na.ts\n#EXTINF:4,\nb.ts\n#EXT-X-ENDLIST\n'
const live = '#EXTM3U\n#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES\n#EXTINF:6,\na.ts\n#EXTINF:4,\nb.ts\n#EXT-X-PART:DURATION=0.5,URI="tail.part"\n'
const partOnlyLive = '#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-PART:DURATION=0.333,URI="tail.part"\n'

describe('HLS browser inspection', () => {
  it('does not let a high-bitrate audio-only variant replace video', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async url => new Response(
      url.endsWith('master.m3u8')
        ? '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=12000000,CODECS="mp4a.40.2"\naudio.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=6000000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2"\nvideo.m3u8\n'
        : vod,
      { status: 200 },
    ))

    const result = await inspectHlsResource({ url: 'https://cdn.test/master.m3u8' }, fetcher)
    expect(result?.height).toBe(1080)
    expect(result?.variants).toHaveLength(1)
    expect(fetcher.mock.calls[1][0]).toBe('https://cdn.test/video.m3u8')
  })

  it('follows the best VOD rendition and estimates its full size', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async url => new Response(
      url.endsWith('master.m3u8') ? master : vod,
      { status: 200 },
    ))

    const result = await inspectHlsResource({
      url: 'https://cdn.test/master.m3u8',
      requestHeaders: {
        Authorization: 'Bearer playback-token',
        Referer: 'https://page.test/watch',
        'User-Agent': 'Captured Browser',
      },
    }, fetcher)

    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls[1][0]).toBe('https://cdn.test/1080/index.m3u8')
    expect(fetcher.mock.calls[0][1].headers).toMatchObject({ authorization: 'Bearer playback-token' })
    expect(fetcher.mock.calls[0][1].headers).toMatchObject({ referer: 'https://page.test/watch' })
    expect(result).toMatchObject({
      inspected: true,
      manifestType: 'master',
      isLive: false,
      duration: 10,
      height: 1080,
      bandwidth: 8_000_000,
      estimatedSize: 10_000_000,
      playbackUrls: [
        'https://cdn.test/1080/a.ts',
        'https://cdn.test/1080/b.ts',
      ],
    })
  })

  it('does not present a live sliding window as total duration or size', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async url => new Response(
      url.endsWith('master.m3u8') ? master : live,
      { status: 200 },
    ))

    const result = await inspectHlsResource({
      url: 'https://cdn.test/master.m3u8',
      estimatedSize: 999,
    }, fetcher)

    expect(result?.duration).toBeUndefined()
    expect(result?.estimatedSize).toBeUndefined()
    expect(result?.variants).toHaveLength(1)
    expect(result).toMatchObject({
      inspected: true,
      manifestType: 'master',
      isLive: true,
      lowLatencyLive: true,
    })
  })

  it('keeps a PART-only live manifest instead of waiting forever for EXTINF', async () => {
    const result = await inspectHlsResource({
      url: 'https://cdn.test/part-only.m3u8',
    }, async () => new Response(partOnlyLive, { status: 200 }))

    expect(result).toMatchObject({
      inspected: true,
      manifestType: 'media',
      isLive: true,
      lowLatencyLive: true,
      partOnlyLive: true,
    })
  })

  it('rejects oversized or non-HLS responses instead of parsing arbitrary bodies', async () => {
    await expect(inspectHlsResource({ url: 'https://cdn.test/huge.m3u8' }, async () => new Response(
      '#EXTM3U\n#EXTINF:1,\na.ts\n',
      { status: 200, headers: { 'content-length': String(3 * 1024 * 1024) } },
    ))).resolves.toBeNull()
    await expect(inspectHlsResource({ url: 'https://cdn.test/fake.m3u8' }, async () => new Response(
      '<html>not a playlist</html>',
      { status: 200 },
    ))).resolves.toBeNull()
  })
})
