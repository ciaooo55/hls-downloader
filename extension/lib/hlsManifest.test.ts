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

  it('totals VOD segment durations and recognizes quality in URLs', () => {
    const live = '#EXTM3U\n#EXTINF:5.5,\na.ts\n#EXTINF:4.5,\nb.ts'
    expect(parseHlsManifest(live, 'https://cdn.test/live.m3u8')).toMatchObject({ duration: 10, isLive: true })
    expect(parseHlsManifest(`${live}\n#EXT-X-ENDLIST`, 'https://cdn.test/vod.m3u8')).toMatchObject({ duration: 10, isLive: false })
    expect(resourceQuality('https://cdn.test/path/1080p/video.m3u8')).toBe('1080p')
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
    })
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

  it('merges provider access fields when a child already has its own query', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8?playlistType=child\n',
      'https://edge.test/live/master.m3u8?pkey=key&psch=v2&playlistType=lowLatency&token=secret',
    )
    expect(info.variants[0].url).toBe(
      'https://edge.test/live/video.m3u8?playlistType=child&pkey=key&psch=v2&token=secret',
    )
  })
})
