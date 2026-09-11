import { describe, expect, it } from 'vitest'

import { parseDashManifest } from './dashInspection'

describe('DASH XML namespace inspection', () => {
  it('parses prefixed MPD elements consistently with manifest sniffing', () => {
    const namespaced = `<?xml version="1.0"?>
<mpd:MPD xmlns:mpd="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT10S">
  <mpd:BaseURL>https://media.test/root/</mpd:BaseURL>
  <mpd:Period>
    <mpd:AdaptationSet contentType="video" mimeType="video/mp4">
      <mpd:SegmentTemplate initialization="$RepresentationID$/init.m4s" media="$RepresentationID$/$Number$.m4s" />
      <mpd:Representation id="v1080" width="1920" height="1080" bandwidth="6000000" />
    </mpd:AdaptationSet>
  </mpd:Period>
</mpd:MPD>`

    expect(parseDashManifest(namespaced, 'https://cdn.test/path/manifest')).toMatchObject({
      inspected: true,
      isLive: false,
      duration: 10,
      width: 1920,
      height: 1080,
      bandwidth: 6_000_000,
      estimatedSize: 7_500_000,
      quality: '最高 1080p',
      playbackUrls: ['https://media.test/root/v1080/init.m4s'],
      playbackPatterns: ['https://media.test/root/v1080/*.m4s'],
    })
  })
})
