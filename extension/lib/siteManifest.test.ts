import { describe, expect, it } from 'vitest'
import { shouldReportMediaResponse } from './manifestSniff'
import { SITE_RESPONSE_LIMIT, bilibiliPageVideoId, bilibiliPlayinfoCandidates, extractBilibiliPlayinfo, isBilibiliAudioTrackMseUrl, isBilibiliFullTrackMseUrl, sameSiteVideoIdentity, shouldInspectSiteResponse, siteManifestCandidates, sitePageVideoId, youtubePageVideoId } from './siteManifest'

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

  it('extracts Bilibili DASH and durl playback metadata across /video/ and /en/video/ pages', () => {
    const cnPage = 'https://www.bilibili.com/video/BV1xx411c7mD/'
    const enPage = 'https://www.bilibili.com/en/video/BV1xx411c7mD/?vd_source=abc'
    const playurlApi = 'https://api.bilibili.com/x/player/wbi/playurl?bvid=BV1xx411c7mD&cid=123'
    const video1080 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30080.m4s?deadline=1900000000'
    const video720 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30064.m4s?deadline=1900000000'
    const audio192 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30280.m4s?deadline=1900000000'

    expect(bilibiliPageVideoId(cnPage)).toBe('BV1xx411c7mD')
    expect(bilibiliPageVideoId(enPage)).toBe('BV1xx411c7mD')
    expect(sitePageVideoId(enPage)).toBe('BV1xx411c7mD')
    expect(shouldInspectSiteResponse(playurlApi, enPage, 'application/json; charset=utf-8')).toBe(true)
    expect(shouldReportMediaResponse(playurlApi, 'application/json')).toBe(false)
    expect(shouldInspectSiteResponse('https://api.bilibili.com/x/web-interface/view', enPage, 'application/json')).toBe(false)

    const dashPayload = {
      code: 0,
      data: {
        timelength: 185000,
        dash: {
          duration: 185,
          video: [
            { id: 64, baseUrl: video720, width: 1280, height: 720, bandwidth: 1_500_000, codecs: 'avc1.64001f' },
            { id: 80, baseUrl: video1080, width: 1920, height: 1080, bandwidth: 3_200_000, codecs: 'avc1.640032' },
          ],
          audio: [
            { id: 30280, baseUrl: audio192, bandwidth: 192_000, codecs: 'mp4a.40.2' },
          ],
        },
      },
    }
    const extracted = siteManifestCandidates(playurlApi, enPage, dashPayload)
    expect(extracted).toHaveLength(1)
    expect(extracted[0]).toMatchObject({
      url: video1080,
      audioUrl: audio192,
      mimeType: 'video/mp4',
      videoId: 'BV1xx411c7mD',
      inspection: {
        inspected: true,
        isLive: false,
        audioUrl: audio192,
        filename: 'BV1xx411c7mD.mp4',
        duration: 185,
        width: 1920,
        height: 1080,
        quality: '1080p',
      },
    })
    expect(extracted[0].inspection?.variants?.map(v => v.height)).toEqual([1080, 720])
    expect(extracted[0].inspection?.variants?.every(v => v.audioUrl === audio192)).toBe(true)
    expect(extracted[0].inspection?.playbackUrls).toEqual(expect.arrayContaining([video1080, video720, audio192]))
    expect(bilibiliPlayinfoCandidates(cnPage, dashPayload)).toEqual(extracted)

    // High-FPS tiering + standard AAC audio preference over flac/dolby + fallback when standard AAC is absent.
    const video1080p60 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30116.m4s?deadline=1900000000'
    const flacAudio = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30251.m4s?deadline=1900000000'
    const dolbyAudio = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30250.m4s?deadline=1900000000'
    const hiResWithStandard = extractBilibiliPlayinfo(cnPage, {
      data: {
        dash: {
          duration: 60,
          video: [
            { id: 116, base_url: video1080p60, width: 1920, height: 1080, frame_rate: '60', bandwidth: 5_800_000, codecs: 'avc1.640032' },
            { id: 80, base_url: video1080, width: 1920, height: 1080, frame_rate: '30', bandwidth: 3_000_000, codecs: 'avc1.640032' },
          ],
          audio: [{ id: 30280, base_url: audio192, bandwidth: 192_000, codecs: 'mp4a.40.2' }],
          flac: { audio: { id: 30251, base_url: flacAudio, bandwidth: 980_000, codecs: 'fLaC' } },
          dolby: { audio: [{ id: 30250, base_url: dolbyAudio, bandwidth: 640_000, codecs: 'ec-3' }] },
        },
      },
    })
    expect(hiResWithStandard[0].audioUrl).toBe(audio192)
    expect(hiResWithStandard[0].inspection?.variants?.map(v => v.quality)).toEqual(['1080p60', '1080p'])

    const flacOnly = extractBilibiliPlayinfo(cnPage, {
      data: {
        dash: {
          duration: 60,
          video: [{ id: 80, base_url: video1080, width: 1920, height: 1080, bandwidth: 3_000_000, codecs: 'avc1.640032' }],
          audio: null,
          flac: { audio: { id: 30251, base_url: flacAudio, bandwidth: 980_000, codecs: 'fLaC' } },
        },
      },
    })
    expect(flacOnly[0].audioUrl).toBe(flacAudio)

    // Progressive durl fallback (common on logged-out /en/video/ pages).
    const mp4Url = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-16.mp4?deadline=1900000000'
    const durlExtracted = bilibiliPlayinfoCandidates(enPage, {
      data: { durl: [{ url: mp4Url, size: 12_345_678, length: 90_000 }] },
    })
    expect(durlExtracted).toHaveLength(1)
    expect(durlExtracted[0]).toMatchObject({
      url: mp4Url,
      mimeType: 'video/mp4',
      videoId: 'BV1xx411c7mD',
      inspection: { inspected: true, duration: 90, estimatedSize: 12_345_678 },
    })

    // Full-track MSE video/audio URL recognition and sameSiteVideoIdentity checks.
    expect(isBilibiliFullTrackMseUrl(video1080)).toBe(true)
    expect(isBilibiliFullTrackMseUrl(audio192)).toBe(false)
    expect(isBilibiliAudioTrackMseUrl(audio192)).toBe(true)
    expect(isBilibiliAudioTrackMseUrl(video1080)).toBe(false)
    expect(isBilibiliFullTrackMseUrl('https://cdn.test/upgcxcode/1/2/3-1-30080.m4s')).toBe(false)
    expect(sameSiteVideoIdentity(cnPage, enPage)).toBe(true)
    expect(sameSiteVideoIdentity(`${cnPage}?p=1`, `${cnPage}?p=2`)).toBe(false)
    expect(sameSiteVideoIdentity('https://www.youtube.com/watch?v=video_1', 'https://www.youtube.com/watch?v=video_1&list=PL2')).toBe(false)
  })
})
