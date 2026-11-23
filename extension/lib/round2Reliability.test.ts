import { describe, expect, it } from 'vitest'
import { parseHlsManifest } from './hlsManifest'
import { RequestChainStore } from './requestChain'
import { compactResources, type MediaResource } from './resources'

describe('Round 2 extension reliability regressions', () => {
  it('matches canonical LL-HLS URLs with responseIdentity against _hls_msn/_hls_part chains', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'req-llhls',
      url: 'https://cdn.example/live/stream.m3u8?token=abc&_hls_msn=42&_hls_part=1',
      tabId: 5,
      frameId: 0,
      type: 'xmlhttprequest',
      method: 'GET',
      initiator: 'https://site.example',
      requestHeaders: [{ name: 'Authorization', value: 'Bearer secret' }],
    })
    store.observeResponse({
      requestId: 'req-llhls',
      url: 'https://cdn.example/live/stream.m3u8?token=abc&_hls_msn=42&_hls_part=1',
      tabId: 5,
      frameId: 0,
      statusCode: 200,
      responseHeaders: [
        { name: 'Content-Type', value: 'application/vnd.apple.mpegurl' },
        { name: 'ETag', value: '"v42"' },
      ],
    })
    const found = store.find({
      url: 'https://cdn.example/live/stream.m3u8?token=abc',
      tabId: 5,
      frameId: 0,
      responseIdentity: {
        'content-type': 'application/vnd.apple.mpegurl',
        etag: '"v42"',
      },
    }, Date.now(), 5, true)
    expect(found?.requestId).toBe('req-llhls')
    expect(found?.requestHeaders.authorization).toBe('Bearer secret')
  })

  it('returns Chromium non-root page and cross-origin iframe request contexts without leaking sibling frames', () => {
    const store = new RequestChainStore()
    // Chromium provides origin-only `initiator` and origin-only Referer on cross-origin key fetches.
    store.observeResponse({
      requestId: 'req-key',
      url: 'https://keys.cdn/aes.key',
      tabId: 10,
      frameId: 0,
      type: 'xmlhttprequest',
      method: 'GET',
      initiator: 'https://site.example',
      requestHeaders: [
        { name: 'Referer', value: 'https://site.example/' },
        { name: 'Authorization', value: 'Bearer key-tok' },
      ],
      statusCode: 200,
    })
    const chromeContexts = store.contextsForPage(10, 'https://site.example/watch/12345', Date.now(), 32, 0)
    expect(chromeContexts.map(c => c.requestId)).toEqual(['req-key'])

    // Cross-origin iframe player (frame 7) vs sibling iframe (frame 8)
    store.observeResponse({
      requestId: 'req-iframe-key',
      url: 'https://drm.embed.net/key',
      tabId: 10,
      frameId: 7,
      type: 'xmlhttprequest',
      method: 'GET',
      documentUrl: 'https://player.embed.net/e/abc',
      requestHeaders: [{ name: 'X-Embed-Auth', value: 'frame7' }],
      statusCode: 200,
    })
    store.observeResponse({
      requestId: 'req-sibling-key',
      url: 'https://drm.embed.net/other-key',
      tabId: 10,
      frameId: 8,
      type: 'xmlhttprequest',
      method: 'GET',
      documentUrl: 'https://player.embed.net/e/xyz',
      requestHeaders: [{ name: 'X-Embed-Auth', value: 'frame8' }],
      statusCode: 200,
    })
    const iframeContexts = store.contextsForPage(
      10,
      'https://blog.example/post/1',
      Date.now(),
      32,
      7,
      'https://player.embed.net/e/abc',
    )
    expect(iframeContexts.map(c => c.requestId)).toEqual(['req-iframe-key'])
  })

  it('preserves webRequest headers and bounds playbackUrlHashes in parseHlsManifest and compactResources', () => {
    const lines = ['#EXTM3U', '#EXT-X-VERSION:3', '#EXT-X-TARGETDURATION:4']
    for (let i = 0; i < 1200; i += 1) {
      lines.push('#EXTINF:4.0,', `https://cdn.example/seg-${i}.ts`)
    }
    const parsed = parseHlsManifest(lines.join('\n'), 'https://cdn.example/live.m3u8')
    expect(parsed.playbackUrlHashes.length).toBeLessThanOrEqual(512)
    expect(parsed.playbackUrlHashes.length).toBe(512)

    const observed: MediaResource = {
      id: 'm1',
      url: 'https://cdn.example/master.m3u8',
      kind: 'hls',
      pageUrl: 'https://site.example/watch',
      tabId: 1,
      frameId: 0,
      statusCode: 200,
      method: 'GET',
      requestHeaders: { authorization: 'Bearer keep-me' },
      replayContext: { method: 'GET', request_id: 'req-master' },
      evidence: ['response_headers'],
      variants: [{ url: 'https://cdn.example/child.m3u8', quality: '1080p' }],
      seenAt: 1000,
    }
    const titleEnrichment: MediaResource = {
      id: 'm1',
      url: 'https://cdn.example/master.m3u8',
      kind: 'hls',
      pageUrl: 'https://site.example/watch',
      tabId: 1,
      frameId: 0,
      title: 'Better Page Title',
      requestHeaders: {},
      seenAt: 2000,
    }
    const child: MediaResource = {
      id: 'c1',
      url: 'https://cdn.example/child.m3u8',
      kind: 'hls',
      pageUrl: 'https://site.example/watch',
      tabId: 1,
      frameId: 0,
      playbackUrlHashes: Array.from({ length: 700 }, (_, i) => `hash-${i}`),
      seenAt: 2100,
    }
    const compacted = compactResources([observed, titleEnrichment, child])
    expect(compacted).toHaveLength(1)
    expect(compacted[0].title).toBe('Better Page Title')
    expect(compacted[0].requestHeaders).toEqual({ authorization: 'Bearer keep-me' })
    expect(compacted[0].statusCode).toBe(200)
    expect(compacted[0].method).toBe('GET')
    expect(compacted[0].replayContext?.request_id).toBe('req-master')
    expect(compacted[0].playbackUrlHashes?.length).toBeLessThanOrEqual(512)
    expect(compacted[0].playbackUrlHashes?.at(-1)).toBe('hash-699')

    // Explicit ambiguity clear (empty request_id) must NOT revive older credentials.
    const ambiguous: MediaResource = {
      ...titleEnrichment,
      requestHeaders: {},
      replayContext: { request_id: '' },
      seenAt: 3000,
    }
    const cleared = compactResources([observed, ambiguous])
    expect(cleared).toHaveLength(1)
    expect(cleared[0].requestHeaders).toEqual({})
    expect(cleared[0].replayContext?.request_id).toBe('')
  })
})
