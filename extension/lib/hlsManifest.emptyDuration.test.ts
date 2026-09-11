import { describe, expect, it } from 'vitest'

import { parseHlsManifest } from './hlsManifest'

describe('HLS EXTINF duration validation', () => {
  it('does not treat an empty EXTINF duration as zero', () => {
    const info = parseHlsManifest(
      '#EXTM3U\n'
      + '#EXTINF:,\n'
      + 'a.ts\n'
      + '#EXTINF:5,\n'
      + 'b.ts\n'
      + '#EXT-X-ENDLIST\n',
      'https://cdn.test/vod.m3u8',
    )

    expect(info).toMatchObject({
      duration: undefined,
      isLive: false,
      playbackUrls: [
        'https://cdn.test/a.ts',
        'https://cdn.test/b.ts',
      ],
    })
  })
})
