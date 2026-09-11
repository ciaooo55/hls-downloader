import { describe, expect, it } from 'vitest'
import { parseHlsManifest, resourceQuality } from './hlsManifest'

describe('HLS metadata', () => {
  it('extracts variants, resolution and bandwidth from a master playlist', () => {
    const info = parseHlsManifest('#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=5200000,RESOLUTION=1920x1080\n1080p/video.m3u8\n', 'https://cdn.test/master.m3u8')
    expect(info.variants).toEqual([{
      url: 'https://cdn.test/1080p/video.m3u8', width: 1920, height: 1080, bandwidth: 5200000, quality: '1080p',
    }])
    expect(info.isLive).toBeUndefined()
  })

  it('rejects non-finite adaptive variant metadata', () => {
    const overflowWidth = '9'.repeat(400)
    const info = parseHlsManifest(
      `#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=Infinity,RESOLUTION=${overflowWidth}x1080\nbad.m3u8\n`
      + '#EXT-X-STREAM-INF:BANDWIDTH=-1,RESOLUTION=1280x720\nnegative.m3u8\n',
      'https://cdn.test/master.m3u8',
    )

    expect(info.variants[0]).toMatchObject({
      width: undefined,
      height: 1080,
      bandwidth: undefined,
      quality: '1080p',
    })
    expect(info.variants[1]).toMatchObject({
      width: 1280,
      height: 720,
      bandwidth: undefined,
      quality: '720p',
    })
  })

  it('does not borrow a later variant URI when a STREAM-INF URI is missing', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n'
      + '#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=640x360\n'
      + '#EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x1080\n'
      + 'high.m3u8\n',
      'https://cdn.test/master.m3u8',
    )

    expect(info.variants).toEqual([{
      url: 'https://cdn.test/high.m3u8',
      width: 1920,
      height: 1080,
      bandwidth: 4_000_000,
      quality: '1080p',
    }])
  })

  it('does not borrow a later segment URI when an EXTINF URI is missing', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n'
      + '#EXTINF:4,\n'
      + '#EXTINF:5,\n'
      + 'b.ts\n'
      + '#EXT-X-ENDLIST\n',
      'https://cdn.test/vod.m3u8',
    )

    expect(info).toMatchObject({
      duration: undefined,
      isLive: false,
      playbackUrls: ['https://cdn.test/b.ts'],
    })
  })

  it('keeps segment tags between EXTINF and the segment URI compatible', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n'
      + '#EXTINF:5,\n'
      + '#EXT-X-BYTERANGE:100@0\n'
      + 'file.ts\n'
      + '#EXT-X-ENDLIST\n',
      'https://cdn.test/vod.m3u8',
    )

    expect(info).toMatchObject({
      duration: 5,
      isLive: false,
      playbackUrls: ['https://cdn.test/file.ts'],
    })
  })

  it('totals VOD segment durations and recognizes quality in URLs', () => {
    const live = '#EXTM3U\n#EXTINF:5.5,\na.ts\n#EXTINF:4.5,\nb.ts'
    expect(parseHlsManifest(live, 'https://cdn.test/live.m3u8')).toMatchObject({ duration: 10, isLive: true })
    expect(parseHlsManifest(`${live}\n#EXT-X-ENDLIST`, 'https://cdn.test/vod.m3u8')).toMatchObject({ duration: 10, isLive: false })
    expect(resourceQuality('https://cdn.test/path/1080p/video.m3u8')).toBe('1080p')
  })

  it('does not synthesize duration from invalid EXTINF values', () => {
    const infinite = '#EXTM3U\n#EXTINF:Infinity,\na.ts\n#EXT-X-ENDLIST\n'
    const negative = '#EXTM3U\n#EXTINF:5,\na.ts\n#EXTINF:-1,\nb.ts\n#EXT-X-ENDLIST\n'
    const zeroThenValid = '#EXTM3U\n#EXTINF:0,\na.ts\n#EXTINF:5,\nb.ts\n#EXT-X-ENDLIST\n'

    expect(parseHlsManifest(infinite, 'https://cdn.test/infinite.m3u8')).toMatchObject({
      duration: undefined,
      isLive: false,
    })
    expect(parseHlsManifest(negative, 'https://cdn.test/negative.m3u8')).toMatchObject({
      duration: undefined,
      isLive: false,
    })
    expect(parseHlsManifest(zeroThenValid, 'https://cdn.test/zero.m3u8')).toMatchObject({
      duration: 5,
      isLive: false,
    })
  })

  it('distinguishes an LL-HLS live playlist from an ordinary event window', () => {
    const ordinary = '#EXTM3U\n#EXTINF:4,\na.ts\n'
    const lowLatency = `${ordinary}#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES\n#EXT-X-PART:DURATION=0.5,URI="a.part"\n`

    expect(parseHlsManifest(ordinary, 'https://cdn.test/event.m3u8').lowLatencyLive).toBe(false)
    expect(parseHlsManifest(lowLatency, 'https://cdn.test/llhls.m3u8')).toMatchObject({
      isLive: true,
      lowLatencyLive: true,
    })
  })

  it('recognizes a PART-only LL-HLS window as live media', () => {
    const partOnly = '#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-PART:DURATION=0.333,URI="p0.m4s"\n#EXT-X-PRELOAD-HINT:TYPE=PART,URI="p1.m4s"\n'

    expect(parseHlsManifest(partOnly, 'https://cdn.test/live.m3u8')).toMatchObject({
      isLive: true,
      lowLatencyLive: true,
      partOnlyLive: true,
      playbackUrls: [
        'https://cdn.test/p0.m4s',
        'https://cdn.test/p1.m4s',
      ],
    })
  })

  it('retains recent segment and init URLs as concrete MSE ownership evidence', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n#EXT-X-MAP:URI="init.mp4"\n#EXTINF:4,\nseg-20.m4s\n#EXT-X-PART:DURATION=0.5,URI="part-21.m4s"\n',
      'https://cdn.test/channel/index.m3u8?token=secret',
    )

    expect(info.playbackUrls).toEqual([
      'https://cdn.test/channel/init.mp4?token=secret',
      'https://cdn.test/channel/seg-20.m4s?token=secret',
      'https://cdn.test/channel/part-21.m4s?token=secret',
    ])
  })

  it('inherits a raw signed playlist token to a relative variant', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8\n',
      'https://edge.test/live/master.m3u8?token=a%2Fb%2Bc&_HLS_msn=5',
    )
    expect(info.variants[0].url).toBe(
      'https://edge.test/live/video.m3u8?token=a%2Fb%2Bc',
    )
  })

  it('inherits terse signatures only when the s/e pair is present', () => {
    const manifest = '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8\n'
    expect(parseHlsManifest(manifest, 'https://edge.test/live/master.m3u8?s=sort').variants[0].url)
      .toBe('https://edge.test/live/video.m3u8')
    expect(parseHlsManifest(manifest, 'https://edge.test/live/master.m3u8?e=event').variants[0].url)
      .toBe('https://edge.test/live/video.m3u8')
    expect(parseHlsManifest(manifest, 'https://edge.test/live/master.m3u8?s=abc&e=123&_t=nonce').variants[0].url)
      .toBe('https://edge.test/live/video.m3u8?s=abc&e=123&_t=nonce')
  })

  it('merges provider access fields when a child already has its own query', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8?playlistType=child\n',
      'https://edge.test/live/master.m3u8?pkey=key&psch=v2&playlistType=lowLatency&token=secret',
    )
    expect(info.variants[0].url).toBe(
      'https://edge.test/live/video.m3u8?playlistType=child&pkey=key&psch=v2&token=secret',
    )
  })

  it('keeps alternate rendition playlists attached to their master', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",URI="audio/live.m3u8"\n'
      + '#EXT-X-STREAM-INF:BANDWIDTH=2000000,AUDIO="a"\nvideo/live.m3u8\n',
      'https://cdn.test/master.m3u8?token=secret',
    )

    expect(info.renditionUrls).toEqual(['https://cdn.test/audio/live.m3u8?token=secret'])
  })
})
