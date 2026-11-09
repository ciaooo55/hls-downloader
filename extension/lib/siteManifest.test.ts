import { describe, expect, it } from 'vitest'
import { shouldReportMediaResponse } from './manifestSniff'
import { SITE_RESPONSE_LIMIT, shouldInspectSiteResponse, siteManifestCandidates, youtubePageVideoId } from './siteManifest'

const page = 'https://www.youtube.com/watch?v=video_1'
const api = 'https://www.youtube.com/youtubei/v1/player?prettyPrint=false'
const hls = 'https://manifest.googlevideo.com/api/manifest/hls_variant/id/video_1?sig=original'
const dash = 'https://www.youtube.com/api/manifest/dash/id/video_1'
const player = (streamingData: Record<string, unknown>, videoId = 'video_1') => ({
  videoDetails: { videoId }, streamingData,
})

describe('scoped site player metadata', () => {
  it('inspects only player JSON on an identified video page, never as a direct download', () => {
    expect(shouldInspectSiteResponse(api, page, 'application/json; charset=utf-8')).toBe(true)
    expect(shouldReportMediaResponse(api, 'application/json')).toBe(false)
    for (const url of [
      'https://www.youtube.com/youtubei/v1/next',
      'https://www.youtube.com/api/events',
      'https://youtube.com.evil.test/youtubei/v1/player',
      'https://notyoutube.com/youtubei/v1/player',
    ]) expect(shouldInspectSiteResponse(url, page, 'application/json')).toBe(false)
    expect(shouldInspectSiteResponse(api, 'https://www.youtube.com/', 'application/json')).toBe(false)
    expect(shouldInspectSiteResponse(api, 'https://elsewhere.test/watch?v=video_1', 'application/json')).toBe(false)
    expect(shouldInspectSiteResponse(api, page, 'text/html')).toBe(false)
  })

  it('extracts only explicit HLS/DASH manifest fields, preserving signed URLs', () => {
    const result = siteManifestCandidates(api, page, JSON.stringify({
      ...player({ hlsManifestUrl: hls, dashManifestUrl: dash, formats: [{ url: 'https://cdn.test/segment.m4s' }] }),
      ads: [{ url: 'https://cdn.test/ad.m3u8' }],
    }))
    expect(result).toEqual([
      { url: hls, mimeType: 'application/vnd.apple.mpegurl', videoId: 'video_1' },
      { url: dash, mimeType: 'application/dash+xml', videoId: 'video_1' },
    ])
    expect(result.every(item => shouldReportMediaResponse(item.url, item.mimeType))).toBe(true)
    expect(siteManifestCandidates(api, page, player({ hlsManifestUrl: hls }))).toEqual([result[0]])
  })

  it('rejects stale SPA/prefetched video responses and missing identities', () => {
    expect(siteManifestCandidates(api, page, player({ hlsManifestUrl: hls }, 'other_video'))).toEqual([])
    expect(siteManifestCandidates(api, page, { streamingData: { hlsManifestUrl: hls } })).toEqual([])
    expect(siteManifestCandidates(api, page.replace('video_1', 'new_video'), player({ hlsManifestUrl: hls }))).toEqual([])
    expect(youtubePageVideoId('https://www.youtube.com/shorts/video_1')).toBe('video_1')
    expect(youtubePageVideoId('https://www.youtube-nocookie.com/embed/video_1')).toBe('video_1')
    expect(youtubePageVideoId('https://www.youtube.com/live/video_1')).toBe('video_1')
  })

  it('does not promote arbitrary hosts, credentials, segments or malformed bodies', () => {
    for (const url of [
      'https://cdn.test/master.m3u8',
      'https://manifest.googlevideo.com.evil.test/master.m3u8',
      'https://user:password@manifest.googlevideo.com/master.m3u8',
      'http://manifest.googlevideo.com/master.m3u8',
      'https://manifest.googlevideo.com/segment.m4s',
      'https://manifest.googlevideo.com/api/events',
    ]) expect(siteManifestCandidates(api, page, player({ hlsManifestUrl: url }))).toEqual([])
    for (const body of ['{', '<html>login</html>', null, [], ' '.repeat(SITE_RESPONSE_LIMIT + 1)]) {
      expect(siteManifestCandidates(api, page, body)).toEqual([])
    }
    expect(siteManifestCandidates(api, page, ")]}'\n" + JSON.stringify(player({ hlsManifestUrl: hls })))).toHaveLength(1)
  })
})
