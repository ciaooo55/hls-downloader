import { describe, expect, it } from 'vitest'
import { parseHlsManifest, resourceQuality } from './hlsManifest'

describe('HLS metadata', () => {
  it('extracts variants, resolution and bandwidth from a master playlist', () => {
    const info = parseHlsManifest('#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=5200000,RESOLUTION=1920x1080\n1080p/video.m3u8\n', 'https://cdn.test/master.m3u8')
    expect(info.variants).toEqual([{
      url: 'https://cdn.test/1080p/video.m3u8', width: 1920, height: 1080, bandwidth: 5200000, quality: '1080p',
    }])
  })

  it('totals VOD segment durations and recognizes quality in URLs', () => {
    expect(parseHlsManifest('#EXTM3U\n#EXTINF:5.5,\na.ts\n#EXTINF:4.5,\nb.ts', 'https://cdn.test/v.m3u8').duration).toBe(10)
    expect(resourceQuality('https://cdn.test/path/1080p/video.m3u8')).toBe('1080p')
  })
})
