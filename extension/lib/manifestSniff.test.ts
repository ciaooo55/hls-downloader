import { describe, expect, it } from 'vitest'
import { detectManifestKind, manifestMimeType, shouldInspectManifestResponse } from './manifestSniff'

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
})
