import { describe, expect, it, vi } from 'vitest'
import { createManifestFetcher, isPrivateOrLocalHost, isSafeInspectionTarget, type ManifestFetcher } from './manifestInspection'
import { parseHlsManifest } from './hlsManifest'
import { inspectHlsResource } from './hlsInspection'
import { parseDashManifest } from './dashInspection'
import { contentDispositionFilename } from './contentDisposition'
import { shouldInspectSiteResponse } from './siteManifest'
import { NativeBridge, type NativePortLike } from './nativeBridge'
import { hostForPage, isHandoffSuppressed } from './handoffSuppression'
import { prefetchSubtitleAudio } from './subtitlePrefetch'
import { compactResources, playerPlaybackResources, type MediaResource } from './resources'

describe('round-3 extension security and reliability regressions', () => {
  it('omits ambient browser credentials on background manifest inspection fetches', async () => {
    const fetcher = vi.fn<ManifestFetcher>(async () => new Response('#EXTM3U\n'))
    await createManifestFetcher({ url: 'https://cdn.test/master.m3u8' }, fetcher)('https://cdn.test/master.m3u8')
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(fetcher.mock.calls[0][1].credentials).toBe('omit')
  })

  it('rejects non-http(s) URIs in HLS and DASH manifest parsers', () => {
    const hls = parseHlsManifest([
      '#EXTM3U',
      '#EXT-X-MEDIA:TYPE=AUDIO,NAME="en",LANGUAGE="en",URI="file:///etc/passwd"',
      '#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080',
      'file:///C:/Windows/win.ini',
      '#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720',
      'https://cdn.test/720.m3u8',
    ].join('\n'), 'https://cdn.test/master.m3u8')
    expect(hls.variants.map(v => v.url)).toEqual(['https://cdn.test/720.m3u8'])
    expect(hls.audioTracks[0]?.url).toBeUndefined()
    expect(hls.renditionUrls).toEqual([])

    const dash = parseDashManifest(`
      <MPD type="static" mediaPresentationDuration="PT10S">
        <Period>
          <BaseURL>file:///C:/Windows/</BaseURL>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="1" bandwidth="1000000" width="1280" height="720">
              <SegmentTemplate initialization="file:///etc/passwd" media="seg-$Number$.m4s" />
            </Representation>
          </AdaptationSet>
        </Period>
      </MPD>
    `, 'https://cdn.test/manifest.mpd')
    expect(dash?.playbackUrls).toEqual([])
    expect(dash?.playbackPatterns).toEqual(['https://cdn.test/seg-*.m4s'])
  })

  it('parses HLS attributes with spaces after commas/colons and quoted commas', () => {
    const info = parseHlsManifest([
      '#EXTM3U',
      '#EXT-X-MEDIA: TYPE=AUDIO, NAME="Director, DEFAULT=YES", LANGUAGE="en", DEFAULT=NO, URI="audio.m3u8"',
      '#EXT-X-STREAM-INF: BANDWIDTH=5000000, RESOLUTION=1920x1080, CODECS="avc1.640028, mp4a.40.2"',
      '1080.m3u8',
    ].join('\n'), 'https://cdn.test/master.m3u8')
    expect(info.variants[0]).toMatchObject({
      url: 'https://cdn.test/1080.m3u8',
      width: 1920,
      height: 1080,
      bandwidth: 5_000_000,
      codecs: 'avc1.640028, mp4a.40.2',
      quality: '1080p',
    })
    expect(info.audioTracks[0]).toMatchObject({
      label: 'Director, DEFAULT=YES',
      language: 'en',
      default: false,
      url: 'https://cdn.test/audio.m3u8',
    })
  })

  it('preserves #EXT-X-MAP and opening segments on long VOD HLS playlists for MSE matching', () => {
    const lines = ['#EXTM3U', '#EXT-X-MAP:URI="init.mp4"']
    for (let i = 0; i < 650; i += 1) {
      lines.push('#EXTINF:4.0,', `segment-${i}.m4s`)
    }
    lines.push('#EXT-X-ENDLIST')
    const info = parseHlsManifest(lines.join('\n'), 'https://cdn.test/vod/index.m3u8')
    expect(info.isLive).toBe(false)
    expect(info.playbackUrls).toContain('https://cdn.test/vod/init.mp4')
    expect(info.playbackUrls).toContain('https://cdn.test/vod/segment-0.m4s')
    expect(info.playbackUrlHashes.length).toBeLessThanOrEqual(512)

    const resource: MediaResource = {
      id: 'vod-1',
      url: 'https://cdn.test/vod/index.m3u8',
      kind: 'hls',
      pageUrl: 'https://site.test/watch',
      title: 'Long VOD',
      seenAt: Date.now(),
      playbackUrls: info.playbackUrls,
      playbackUrlHashes: info.playbackUrlHashes,
    }
    const matched = playerPlaybackResources([resource], {
      sourceUrls: ['blob:https://site.test/abc'],
      startedAt: Date.now(),
      mseResourceUrls: ['https://cdn.test/vod/init.mp4', 'https://cdn.test/vod/segment-0.m4s'],
    })
    expect(matched).toHaveLength(1)
  })

  it('keeps VP9 (vp09) and Dolby Vision (dvh1) 4K HLS variants alongside avc1 fallbacks', async () => {
    const master = [
      '#EXTM3U',
      '#EXT-X-STREAM-INF:BANDWIDTH=15000000,RESOLUTION=3840x2160,CODECS="dvh1.05.06,ec-3"',
      '4k-dv.m3u8',
      '#EXT-X-STREAM-INF:BANDWIDTH=12000000,RESOLUTION=3840x2160,CODECS="vp09.00.51.08,mp4a.40.2"',
      '4k-vp9.m3u8',
      '#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2"',
      '1080.m3u8',
    ].join('\n')
    const result = await inspectHlsResource({ url: 'https://cdn.test/master.m3u8' }, async url => {
      if (url.endsWith('master.m3u8')) return new Response(master)
      return new Response('#EXTM3U\n#EXTINF:6.0,\nseg0.m4s\n#EXT-X-ENDLIST\n')
    })
    expect(result?.height).toBe(2160)
    expect(result?.variants.map(v => v.url)).toEqual([
      'https://cdn.test/4k-dv.m3u8',
      'https://cdn.test/4k-vp9.m3u8',
      'https://cdn.test/1080.m3u8',
    ])
  })

  it('handles DASH XML comments, multi-period deduplication, $Bandwidth$, format tags, and literal BaseURL', () => {
    const mpd = `
      <MPD type="static" mediaPresentationDuration="PT60S">
        <!-- <Period><BaseURL>https://commented.invalid/</BaseURL></Period> -->
        <Period id="p1">
          <AdaptationSet mimeType="audio/mp4" lang="en" label="English">
            <Representation id="a1" bandwidth="128000" />
          </AdaptationSet>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="v1" bandwidth="2000000" width="1920" height="1080">
              <SegmentTemplate initialization="init-$RepresentationID$-$Bandwidth$.m4s" media="seg-$Number%d$.m4s" />
            </Representation>
            <Representation id="v2" bandwidth="800000" width="1280" height="720">
              <BaseURL>files/literal-&#38;amp;$$1.mp4</BaseURL>
            </Representation>
          </AdaptationSet>
        </Period>
        <Period id="p2">
          <AdaptationSet mimeType="audio/mp4" lang="en" label="English">
            <Representation id="a2" bandwidth="128000" />
          </AdaptationSet>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="v1b" bandwidth="2000000" width="1920" height="1080" />
          </AdaptationSet>
        </Period>
      </MPD>
    `
    const parsed = parseDashManifest(mpd, 'https://cdn.test/manifest.mpd')
    expect(parsed).not.toBeNull()
    expect(parsed!.variants).toHaveLength(2)
    expect(parsed!.audioTracks).toHaveLength(1)
    expect(parsed!.playbackUrls).toContain('https://cdn.test/init-v1-2000000.m4s')
    expect(parsed!.playbackUrls).toContain('https://cdn.test/files/literal-&amp;$$1.mp4')
    expect(parsed!.playbackPatterns).toContain('https://cdn.test/seg-*.m4s')
  })

  it('parses Content-Disposition quoted semicolons, UTF-8 surrogate pairs, and ISO-8859-1 bytes', () => {
    expect(contentDispositionFilename('attachment; filename="notes; filename*=UTF-8\'\'spoof.txt"'))
      .toBe('notes; filename*=UTF-8\'\'spoof.txt')
    expect(contentDispositionFilename("attachment; filename*=UTF-8''🎬.mp4"))
      .toBe('🎬.mp4')
    expect(contentDispositionFilename("attachment; filename*=ISO-8859-1''café.mp4"))
      .toBe('café.mp4')
  })

  it('accepts www.youtube-nocookie.com player metadata responses', () => {
    expect(shouldInspectSiteResponse(
      'https://www.youtube-nocookie.com/youtubei/v1/player',
      'https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ',
      'application/json',
    )).toBe(true)
  })

  it('preserves www. host for handoff suppression matching and restoration', () => {
    const pageUrl = 'https://www.example.com/downloads'
    expect(hostForPage(pageUrl)).toBe('www.example.com')
    expect(isHandoffSuppressed([{ host: 'www.example.com', kind: 'file' }], pageUrl, 'file')).toBe(true)
  })

  it('falls back to accepted media_push response when follow-up admission status check returns error envelope', async () => {
    let messageHandler: ((msg: unknown) => void) | undefined
    const posted: Array<Record<string, unknown>> = []
    const port: NativePortLike = {
      postMessage(message) {
        const rec = message as Record<string, unknown>
        posted.push(rec)
        queueMicrotask(() => {
          if (rec.op === 'media_push') {
            messageHandler?.({ ok: true, id: 'push-1', status: 'pending', __request_id: rec.__request_id })
          } else if (rec.op === 'media_push_status') {
            messageHandler?.({ ok: false, error: 'Core pipe read: broken pipe', __request_id: rec.__request_id })
          }
        })
      },
      disconnect: vi.fn(),
      onMessage: { addListener(fn) { messageHandler = fn } },
      onDisconnect: { addListener() {} },
    }
    const bridge = new NativeBridge(() => port)
    const result = await bridge.request({ op: 'media_push', kind: 'cast' }, 5_000, 0)
    expect(result).toMatchObject({ ok: true, id: 'push-1', status: 'pending' })
    expect(posted.map(m => m.op)).toEqual(['media_push', 'media_push_status'])
  })

  it('starts new-epoch subtitle prefetch immediately after reset() even while old send is pending', async () => {
    let releaseOldSend!: () => void
    let currentPos = 0
    const preparedPositions: number[] = []
    const pcm = btoa('\0'.repeat(64_000)) // 2s silence
    const controller = prefetchSubtitleAudio({
      epoch: 0,
      position: 0,
      duration: 1200,
      clock: () => ({ position: currentPos, running: true }),
      prepare: async pos => {
        preparedPositions.push(pos)
        return { audio: pcm, position: pos, duration: 2 }
      },
      send: async (_audio, _pos, ep) => {
        if (ep === 0) {
          await new Promise<void>(resolve => { releaseOldSend = resolve })
        }
      },
      commit: async () => {},
      ready: () => true,
      failed: () => {},
    })
    await vi.waitFor(() => expect(releaseOldSend).toBeTypeOf('function'))
    currentPos = 600
    controller.reset(1, 600)
    await vi.waitFor(() => expect(preparedPositions).toContain(600))
    controller.stop()
    releaseOldSend()
  })

  it('blocks public-page background inspection of loopback, LAN, and metadata targets', () => {
    expect(isPrivateOrLocalHost('localhost')).toBe(true)
    expect(isPrivateOrLocalHost('sub.localhost')).toBe(true)
    expect(isPrivateOrLocalHost('127.0.0.1')).toBe(true)
    expect(isPrivateOrLocalHost('169.254.169.254')).toBe(true)
    expect(isPrivateOrLocalHost('10.0.0.1')).toBe(true)
    expect(isPrivateOrLocalHost('172.16.5.1')).toBe(true)
    expect(isPrivateOrLocalHost('192.168.1.10')).toBe(true)
    expect(isPrivateOrLocalHost('[::1]')).toBe(true)
    expect(isPrivateOrLocalHost('[::ffff:7f00:1]')).toBe(true)
    expect(isPrivateOrLocalHost('[fe80::1]')).toBe(true)
    expect(isPrivateOrLocalHost('[fd12:3456::1]')).toBe(true)
    expect(isPrivateOrLocalHost('nas')).toBe(true)
    expect(isPrivateOrLocalHost('metadata.google.internal')).toBe(true)
    expect(isPrivateOrLocalHost('printer.home.arpa')).toBe(true)
    expect(isPrivateOrLocalHost('[64:ff9b::127.0.0.1]')).toBe(true)
    expect(isPrivateOrLocalHost('[fec0::1]')).toBe(true)
    expect(isPrivateOrLocalHost('198.18.0.1')).toBe(true)
    expect(isPrivateOrLocalHost('cdn.example.com')).toBe(false)

    expect(isSafeInspectionTarget('http://127.0.0.1:8080/live.m3u8', 'https://evil.example/')).toBe(false)
    expect(isSafeInspectionTarget('http://169.254.169.254/latest.m3u8', 'https://evil.example/')).toBe(false)
    expect(isSafeInspectionTarget('http://[::1]:9000/a.m3u8', 'https://evil.example/')).toBe(false)
    expect(isSafeInspectionTarget('http://2130706433/a.m3u8', 'https://evil.example/')).toBe(false)
    expect(isSafeInspectionTarget('http://nas/a.m3u8', 'https://evil.example/')).toBe(false)
    expect(isSafeInspectionTarget('http://metadata.google.internal/a.m3u8', '')).toBe(false)
    expect(isSafeInspectionTarget('file:///etc/passwd', 'https://evil.example/')).toBe(false)
    expect(isSafeInspectionTarget('https://cdn-b.example/child.m3u8', 'https://site.example/')).toBe(true)
    expect(isSafeInspectionTarget('http://127.0.0.1:8080/live.m3u8', 'http://localhost:3000/watch')).toBe(true)
  })

  it('substitutes printf-formatted $Bandwidth%06d$ tokens in DASH templates', () => {
    const mpd = `<MPD type="static" mediaPresentationDuration="PT10S"><Period><AdaptationSet mimeType="video/mp4"><SegmentTemplate initialization="init-$Bandwidth%06d$.mp4" media="seg-$Bandwidth%06d$-$Number%04d$.m4s"/><Representation id="v1" bandwidth="5000" width="1280" height="720"/></AdaptationSet></Period></MPD>`
    const parsed = parseDashManifest(mpd, 'https://cdn.test/live/manifest.mpd')
    expect(parsed?.playbackUrls).toContain('https://cdn.test/live/init-005000.mp4')
    expect(parsed?.playbackPatterns).toContain('https://cdn.test/live/seg-*-*.m4s')
  })

  it('tolerates malformed non-array playbackUrlHashes during compactResources without crashing', () => {
    const malformed = {
      id: 'bad-hashes',
      url: 'https://cdn.test/master.m3u8',
      kind: 'hls' as const,
      seenAt: 1000,
      playbackUrlHashes: 123 as unknown as string[],
    }
    const compacted = compactResources([malformed, { ...malformed, seenAt: 2000 }])
    expect(compacted).toHaveLength(1)
    expect(compacted[0].playbackUrlHashes).toBeUndefined()
  })
})
