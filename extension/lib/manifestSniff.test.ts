import { describe, expect, it } from 'vitest'
import { detectManifestKind, isCommonMediaStreamUrl, manifestMimeType, shouldInspectManifestResponse, shouldReportMediaResponse } from './manifestSniff'

describe('extensionless manifest sniffing', () => {
  it('only opts into the bounded clone for manifest-like responses', () => {
    expect(shouldInspectManifestResponse('https://cdn.test/live/playlist?id=1', 'application/octet-stream')).toBe(true)
    expect(shouldInspectManifestResponse('https://cdn.test/assets/movie.mp4', 'video/mp4')).toBe(false)
    expect(shouldInspectManifestResponse('https://cdn.test/get?id=1', 'application/vnd.apple.mpegurl')).toBe(true)
  })

  it('recognizes HLS and namespaced DASH prefixes', () => {
    expect(detectManifestKind('\ufeff  #EXTM3U\n#EXT-X-TARGETDURATION:2')).toBe('hls')
    expect(detectManifestKind('<mpd:MPD xmlns:mpd="urn:mpeg:dash:schema:mpd:2011">')).toBe('dash')
    expect(detectManifestKind('<html>login</html>')).toBeNull()
    expect(manifestMimeType('hls')).toBe('application/vnd.apple.mpegurl')
    expect(manifestMimeType('dash')).toBe('application/dash+xml')
  })

  it('does not wake the isolated media UI for ordinary page traffic', () => {
    expect(shouldReportMediaResponse('https://api.test/events', 'application/json')).toBe(false)
    expect(shouldReportMediaResponse('https://static.test/app.js', 'text/javascript')).toBe(false)
    expect(shouldReportMediaResponse('https://cdn.test/movie.mp4?token=1', 'application/octet-stream')).toBe(true)
    expect(shouldReportMediaResponse('https://cdn.test/media?id=1', 'video/mp4; charset=binary')).toBe(true)
    expect(shouldReportMediaResponse('https://cdn.test/live/playlist?id=1', 'application/octet-stream')).toBe(true)
    expect(shouldReportMediaResponse('https://rr1.googlevideo.test/videoplayback?expire=1&mime=video%2Fmp4&itag=18', '')).toBe(true)
    expect(shouldReportMediaResponse('https://cdn.test/videoplayback?id=1', 'application/octet-stream')).toBe(true)
  })

  it('recognizes current-site extensionless media streams without promoting APIs', () => {
    expect(isCommonMediaStreamUrl('https://rr1.googlevideo.test/videoplayback?expire=1&mime=video%2Fmp4&itag=18')).toBe(true)
    expect(isCommonMediaStreamUrl('https://cdn.test/get?mime=audio%2Fwebm&itag=251')).toBe(true)
    expect(isCommonMediaStreamUrl('https://api.bilibili.test/x/player/playurl?cid=1')).toBe(false)
    expect(isCommonMediaStreamUrl('https://cdn.test/upgcxcode/1/2/3-1-30080.m4s')).toBe(false)
  })
})
