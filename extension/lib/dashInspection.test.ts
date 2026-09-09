import { describe, expect, it, vi } from 'vitest'

import { inspectDashResource, parseDashManifest, type DashManifestFetcher } from './dashInspection'

const vod = `<?xml version="1.0"?>
<MPD type="static" mediaPresentationDuration="PT1M30S">
  <Period>
    <AdaptationSet contentType="video" mimeType="video/mp4">
      <Representation id="v720" width="1280" height="720" bandwidth="2500000" />
      <Representation id="v1080" width="1920" height="1080" bandwidth="6000000">
        <SegmentTemplate initialization="video/$RepresentationID$/init.m4s" media="video/$RepresentationID$/$Number%05d$.m4s" />
      </Representation>
    </AdaptationSet>
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation id="audio" bandwidth="192000" />
    </AdaptationSet>
  </Period>
</MPD>`

describe('DASH browser inspection', () => {
  it('extracts the best video representation, size and playback hints', () => {
    expect(parseDashManifest(vod, 'https://cdn.test/path/manifest.mpd?token=x')).toMatchObject({
      inspected: true,
      isLive: false,
      duration: 90,
      width: 1920,
      height: 1080,
      bandwidth: 6_000_000,
      estimatedSize: 69_660_000,
      quality: '最高 1080p',
      playbackUrls: [
        'https://cdn.test/path/video/v1080/init.m4s',
      ],
      playbackPatterns: ['https://cdn.test/path/video/v1080/*.m4s'],
    })
  })

  it('does not invent a finite size for a dynamic MPD', async () => {
    const fetcher = vi.fn<DashManifestFetcher>(async () => new Response(
      vod.replace('type="static"', 'type="dynamic"'),
      { status: 200, headers: { 'content-length': String(vod.length) } },
    ))
    const result = await inspectDashResource({
      url: 'https://cdn.test/live.mpd',
      requestHeaders: { Authorization: 'Bearer media', Referer: 'https://page.test/watch' },
    }, fetcher)

    expect(result?.isLive).toBe(true)
    expect(result?.duration).toBeUndefined()
    expect(result?.estimatedSize).toBeUndefined()
    expect(fetcher.mock.calls[0][1].headers).toMatchObject({ authorization: 'Bearer media' })
    expect(fetcher.mock.calls[0][1].headers).toMatchObject({ referer: 'https://page.test/watch' })
  })

  it('uses only the selected video hierarchy and resolves nested BaseURL values', () => {
    const nested = `<MPD type="static" mediaPresentationDuration="PT10S">
      <BaseURL>https://media.test/root/</BaseURL><Period><BaseURL>period/</BaseURL>
      <AdaptationSet contentType="video"><BaseURL>video/</BaseURL>
        <SegmentTemplate initialization="$RepresentationID$/init.m4s" media="$RepresentationID$/$Number$.m4s" />
        <Representation id="v1" width="1280" height="720" bandwidth="2000000" />
      </AdaptationSet>
      <AdaptationSet contentType="audio"><BaseURL>audio/</BaseURL>
        <SegmentTemplate initialization="$RepresentationID$/init.m4s" media="$RepresentationID$/$Number$.m4s" />
        <Representation id="a1" bandwidth="128000" />
      </AdaptationSet></Period></MPD>`

    const result = parseDashManifest(nested, 'https://page.test/manifest.mpd')
    expect(result?.playbackUrls).toEqual(['https://media.test/root/period/video/v1/init.m4s'])
    expect(result?.playbackPatterns).toEqual(['https://media.test/root/period/video/v1/*.m4s'])
    expect(result?.playbackUrls.join('\n')).not.toContain('/audio/')
  })

  it('keeps a representation BaseURL file as exact playback evidence', () => {
    const direct = `<MPD type="static" mediaPresentationDuration="PT5S"><Period>
      <AdaptationSet contentType="video"><Representation id="v" width="640" height="360" bandwidth="500000">
        <BaseURL>files/movie-v.mp4</BaseURL>
      </Representation></AdaptationSet></Period></MPD>`

    expect(parseDashManifest(direct, 'https://cdn.test/path/manifest.mpd')?.playbackUrls)
      .toEqual(['https://cdn.test/path/files/movie-v.mp4'])
  })
})
