import { describe, expect, it } from 'vitest'

import { inheritManifestAccessQuery, removeRawQueryParameters } from './urlQuery'

describe('removeRawQueryParameters', () => {
  it('removes LL-HLS reload cursors without normalizing the signed query', () => {
    const url = 'https://cdn.test/live/index.m3u8?token=abc%2Fdef%3D&_hls_msn=272&_hls_part=4.2&_hls_skip=v2'
    expect(removeRawQueryParameters(url, new Set(['_hls_msn', '_hls_part', '_hls_skip'])))
      .toBe('https://cdn.test/live/index.m3u8?token=abc%2Fdef%3D')
  })

  it('matches parameter names case-insensitively and after percent decoding', () => {
    expect(removeRawQueryParameters('https://cdn.test/a.m3u8?_HLS_MSN=1&keep=1', new Set(['_hls_msn'])))
      .toBe('https://cdn.test/a.m3u8?keep=1')
    expect(removeRawQueryParameters('https://cdn.test/a.m3u8?%5Fhls%5Fmsn=1&keep=1', new Set(['_hls_msn'])))
      .toBe('https://cdn.test/a.m3u8?keep=1')
  })

  it('keeps the fragment and unrelated parameters intact', () => {
    expect(removeRawQueryParameters('https://cdn.test/a.m3u8?_hls_skip=v2#t=10', new Set(['_hls_skip'])))
      .toBe('https://cdn.test/a.m3u8#t=10')
    expect(removeRawQueryParameters('https://cdn.test/a.m3u8?Policy=x&key-pair-id=y', new Set(['token'])))
      .toBe('https://cdn.test/a.m3u8?Policy=x&key-pair-id=y')
  })

  it('passes URLs without a query through untouched', () => {
    expect(removeRawQueryParameters('https://cdn.test/a.m3u8', new Set(['_hls_msn'])))
      .toBe('https://cdn.test/a.m3u8')
    expect(removeRawQueryParameters('https://cdn.test/a.m3u8#frag', new Set(['_hls_msn'])))
      .toBe('https://cdn.test/a.m3u8#frag')
  })

  it('does not treat a value that merely contains a removed name as that name', () => {
    expect(removeRawQueryParameters('https://cdn.test/a.m3u8?redirect=_hls_msn', new Set(['_hls_msn'])))
      .toBe('https://cdn.test/a.m3u8?redirect=_hls_msn')
  })
})

describe('inheritManifestAccessQuery', () => {
  const master = 'https://cdn.test/hls/master.m3u8?token=signed-value&Policy=abc&Key-Pair-Id=apk&expires=1893456000'

  it('carries bearer and signature fields to a same-origin child URI', () => {
    const child = 'https://cdn.test/hls/1080p/video.m3u8?foo=1'
    expect(inheritManifestAccessQuery(master, child))
      .toBe('https://cdn.test/hls/1080p/video.m3u8?foo=1&token=signed-value&Policy=abc&Key-Pair-Id=apk&expires=1893456000')
  })

  it('keeps the child fragment and works when the child has no query of its own', () => {
    expect(inheritManifestAccessQuery(master, 'https://cdn.test/hls/720p/video.m3u8#seg'))
      .toBe('https://cdn.test/hls/720p/video.m3u8?token=signed-value&Policy=abc&Key-Pair-Id=apk&expires=1893456000#seg')
  })

  it('refuses to cross origins even when the parameter names match', () => {
    const child = 'https://other.test/hls/video.m3u8'
    expect(inheritManifestAccessQuery(master, child)).toBe(child)
  })

  it('returns the child unchanged when the master has no access query', () => {
    const child = 'https://cdn.test/hls/video.m3u8?foo=1'
    expect(inheritManifestAccessQuery('https://cdn.test/hls/master.m3u8', child)).toBe(child)
  })

  it('never inherits LL-HLS reload cursors or overwrites child parameters', () => {
    const liveMaster = 'https://cdn.test/live/master.m3u8?token=abc&_hls_msn=99&_hls_part=1.0&_hls_skip=v2'
    const child = 'https://cdn.test/live/chunk.m3u8?token=child-own'
    expect(inheritManifestAccessQuery(liveMaster, child)).toBe(child)
  })

  it('inherits a terse s/e signature pair but not an unpaired half', () => {
    const signed = 'https://cdn.test/hls/master.m3u8?s=abc123&e=1893456000'
    expect(inheritManifestAccessQuery(signed, 'https://cdn.test/hls/video.m3u8'))
      .toBe('https://cdn.test/hls/video.m3u8?s=abc123&e=1893456000')
    const half = 'https://cdn.test/hls/master.m3u8?s=abc123'
    expect(inheritManifestAccessQuery(half, 'https://cdn.test/hls/video.m3u8'))
      .toBe('https://cdn.test/hls/video.m3u8')
  })

  it('falls back to the child when either URL cannot be parsed', () => {
    expect(inheritManifestAccessQuery('not-a-url', 'https://cdn.test/hls/video.m3u8'))
      .toBe('https://cdn.test/hls/video.m3u8')
    expect(inheritManifestAccessQuery('https://cdn.test/hls/master.m3u8?token=x', 'not-a-url'))
      .toBe('not-a-url')
  })
})
