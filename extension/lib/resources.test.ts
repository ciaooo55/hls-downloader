import { describe, expect, it } from 'vitest'
import {
  classifyDownload,
  classifyResource,
  compactResources,
  isGenericMediaName,
  isUsefulResource,
  matchesDownloadClick,
  mergeResources,
  pageResourceKey,
  replayableRequestHeaders,
  resourceFingerprint,
  resourceRequestIdentity,
  shouldTakeover,
  suggestedResourceFilename,
  visibleMediaResources,
  type MediaResource,
} from './resources'

function resource(overrides: Partial<MediaResource> = {}): MediaResource {
  return {
    id: 'resource',
    url: 'https://cdn.test/movie.mp4',
    kind: 'media',
    seenAt: Date.now(),
    ...overrides,
  }
}

describe('resource rules', () => {
  it('filters HLS segments but retains manifests', () => {
    expect(classifyResource('https://cdn.test/a.m3u8')).toBe('hls')
    expect(classifyResource('https://cdn.test/0001.ts')).toBeNull()
    expect(classifyResource('https://cdn.test/file.torrent?token=1')).toBe('file')
    expect(classifyResource('https://cdn.test/get?id=1', 'application/x-bittorrent')).toBe('file')
  })
  it('deduplicates resources', () => {
    const item = { id: '1', url: 'https://a.test/v.mp4', kind: 'media' as const, seenAt: Date.now() }
    expect(mergeResources([item], { ...item, size: 20 })).toHaveLength(1)
    expect(mergeResources([item], { ...item, size: 20 })[0].size).toBe(20)
  })
  it('rejects failed responses and irrelevant request methods', () => {
    const successfulGet = resource({ statusCode: 206, method: 'get', size: 20 * 1024 * 1024 })
    expect(isUsefulResource(successfulGet)).toBe(true)
    expect(isUsefulResource({ ...successfulGet, statusCode: 404 })).toBe(false)
    expect(isUsefulResource({ ...successfulGet, statusCode: 500 })).toBe(false)
    expect(isUsefulResource({ ...successfulGet, method: 'POST' })).toBe(true)
    expect(isUsefulResource({ ...successfulGet, method: 'HEAD' })).toBe(false)
    expect(isUsefulResource({ ...successfulGet, method: 'OPTIONS' })).toBe(false)
  })
  it('filters media fragments identified by MIME type or init/segment URLs', () => {
    expect(isUsefulResource(resource({
      url: 'https://cdn.test/delivery?id=42',
      mimeType: 'video/mp2t; charset=binary',
    }))).toBe(false)
    expect(isUsefulResource(resource({
      url: 'https://cdn.test/audio/chunk?id=42',
      mimeType: 'audio/aac',
    }))).toBe(false)
    expect(isUsefulResource(resource({
      url: 'https://cdn.test/vod/init.mp4',
      mimeType: 'video/mp4',
      size: 32 * 1024,
    }))).toBe(false)
    expect(isUsefulResource(resource({
      url: 'https://cdn.test/vod/segment-000042.mp4',
      mimeType: 'video/mp4',
      size: 512 * 1024,
    }))).toBe(false)
    expect(isUsefulResource(resource({
      url: 'https://cdn.test/vod/movie.mp4',
      mimeType: 'video/mp4',
      size: 20 * 1024 * 1024,
    }))).toBe(true)
  })
  it('filters non-video HLS and DASH tracks before they reach the media panel', () => {
    expect(isUsefulResource(resource({ kind: 'hls', url: 'https://cdn.test/tracks/audio/master.m3u8' }))).toBe(false)
    expect(isUsefulResource(resource({ kind: 'dash', url: 'https://cdn.test/subtitles/track.mpd' }))).toBe(false)
    expect(isUsefulResource(resource({ kind: 'hls', url: 'https://cdn.test/ads/preroll.m3u8' }))).toBe(false)
    expect(isUsefulResource(resource({ kind: 'dash', url: 'https://cdn.test/video/manifest.mpd?track=audio' }))).toBe(false)
    expect(isUsefulResource(resource({ kind: 'hls', url: 'https://cdn.test/video/master.m3u8' }))).toBe(true)
    expect(isUsefulResource(resource({ kind: 'hls', url: 'https://cdn.test/adventure/master.m3u8' }))).toBe(true)
  })
  it('stably deduplicates refreshed signed URLs while preserving meaningful query parameters', () => {
    const now = Date.now()
    const previous = resource({
      id: 'old-signature',
      kind: 'hls',
      url: 'https://cdn.test/master.m3u8?quality=1080&token=old&expires=100',
      seenAt: now - 1_000,
    })
    const refreshed = resource({
      id: 'new-signature',
      kind: 'hls',
      url: 'https://cdn.test/master.m3u8?X-Amz-Signature=new&quality=1080&X-Amz-Expires=900',
      seenAt: now,
    })

    expect(resourceFingerprint(previous)).toBe(resourceFingerprint(refreshed))
    expect(resourceFingerprint(previous)).not.toBe(resourceFingerprint({
      ...refreshed,
      url: 'https://cdn.test/master.m3u8?quality=720&token=new',
    }))
    const merged = mergeResources([previous], refreshed)
    expect(merged).toHaveLength(1)
    expect(merged[0]).toMatchObject({ id: 'new-signature', url: refreshed.url, seenAt: now })
  })
  it('folds captured HLS variants into their master manifest', () => {
    const master = resource({
      id: 'master',
      kind: 'hls',
      url: 'https://cdn.test/master.m3u8?token=master',
      variants: [
        { url: 'https://cdn.test/1080p/index.m3u8?token=from-manifest', height: 1080 },
        { url: 'https://cdn.test/720p/index.m3u8?token=from-manifest', height: 720 },
      ],
    })
    const high = resource({
      id: '1080p-child',
      kind: 'hls',
      url: 'https://cdn.test/1080p/index.m3u8?token=observed',
    })
    const medium = resource({
      id: '720p-child',
      kind: 'hls',
      url: 'https://cdn.test/720p/index.m3u8?token=refreshed',
    })

    expect(compactResources([high, master, medium])).toEqual([master])
  })
  it('sorts useful resources by relevance and recency', () => {
    const now = Date.now()
    const master = resource({
      id: 'master', kind: 'hls', url: 'https://cdn.test/master.m3u8', seenAt: now - 10_000,
      variants: [{ url: 'https://cdn.test/high.m3u8', height: 1080 }],
    })
    const recentHls = resource({ id: 'recent-hls', kind: 'hls', url: 'https://cdn.test/recent.m3u8', seenAt: now })
    const olderHls = resource({ id: 'older-hls', kind: 'hls', url: 'https://cdn.test/older.m3u8', seenAt: now - 1_000 })
    const largeMedia = resource({
      id: 'large-media', url: 'https://cdn.test/movie.mp4', seenAt: now + 1_000,
      size: 100 * 1024 * 1024, duration: 3_600, height: 1080,
    })

    expect(compactResources([largeMedia, olderHls, master, recentHls]).map(item => item.id)).toEqual([
      'master', 'recent-hls', 'older-hls', 'large-media',
    ])
  })
  it('limits visible media resources and omits file noise when video exists', () => {
    const now = Date.now()
    const streams = Array.from({ length: 10 }, (_, index) => resource({
      id: `stream-${index}`,
      kind: 'hls',
      url: `https://cdn.test/stream-${index}.m3u8`,
      seenAt: now + index,
    }))
    const file = resource({
      id: 'unrelated-file',
      kind: 'file',
      url: 'https://cdn.test/unrelated.zip',
      seenAt: now + 100,
    })

    expect(visibleMediaResources([...streams, file])).toHaveLength(8)
    expect(visibleMediaResources([...streams, file]).map(item => item.id)).toEqual([
      'stream-9', 'stream-8', 'stream-7', 'stream-6', 'stream-5', 'stream-4', 'stream-3', 'stream-2',
    ])
    expect(visibleMediaResources([...streams, file], 3).map(item => item.id)).toEqual([
      'stream-9', 'stream-8', 'stream-7',
    ])
  })
  it('honors Alt bypass and Ctrl force', () => {
    const base = { url: 'https://a.test/file.zip', size: 20, enabled: true, minimumBytes: 10, excludedHosts: [], explicitClick: true }
    expect(shouldTakeover({ ...base, altBypass: true })).toBe(false)
    expect(shouldTakeover({ ...base, enabled: false, ctrlForce: true })).toBe(true)
    expect(shouldTakeover({ ...base, size: 9 })).toBe(false)
    expect(shouldTakeover({ ...base, size: 0 })).toBe(true)
    expect(shouldTakeover({ ...base, url: 'https://sub.blocked.test/file.zip', excludedHosts: ['blocked.test'] })).toBe(false)
  })
  it('takes over unknown-size downloads and classifies response filenames', () => {
    expect(classifyDownload('https://cdn.test/get?id=1', 'application/octet-stream', 'setup.exe')).toBe('file')
    expect(shouldTakeover({
      url: 'https://cdn.test/get?id=1', filename: 'archive.zip', size: -1,
      enabled: true, minimumBytes: 1024 * 1024, excludedHosts: [], explicitClick: true,
    })).toBe(true)
  })
  it('uses a page title when an HLS manifest has a generic filename', () => {
    expect(suggestedResourceFilename({
      kind: 'hls',
      url: 'https://cdn.test/video.m3u8?token=1',
      pageUrl: 'https://site.test/watch/episode-12',
      title: '第十二集：重新出发',
      filename: 'video.m3u8',
    })).toBe('第十二集：重新出发')
    expect(suggestedResourceFilename({
      kind: 'hls',
      url: 'https://cdn.test/series/episode-07.m3u8',
      title: '网页标题',
      filename: 'episode-07.m3u8',
    })).toBe('episode-07')
    expect(isGenericMediaName('1080p HLS 视频流')).toBe(true)
    expect(isGenericMediaName('video_1080p.m3u8')).toBe(true)
    expect(isGenericMediaName('master-high.m3u8')).toBe(true)
    expect(isGenericMediaName('HLS 720p')).toBe(true)
    expect(suggestedResourceFilename({
      kind: 'hls',
      url: 'https://cdn.test/master.m3u8',
      pageUrl: 'https://site.test/watch/real-title?episode=12#player',
      title: '1080p HLS 视频流',
      filename: 'master.m3u8',
    })).toBe('real-title')
  })
  it('isolates captured resources by current page inside the same tab', () => {
    expect(pageResourceKey(9, 'https://site.test/watch/1#player')).toBe(pageResourceKey(9, 'https://site.test/watch/1'))
    expect(pageResourceKey(9, 'https://site.test/watch/1')).not.toBe(pageResourceKey(9, 'https://site.test/watch/2'))
    expect(pageResourceKey(9, 'https://site.test/watch?id=1')).not.toBe(pageResourceKey(9, 'https://site.test/watch?id=2'))
    expect(pageResourceKey(9, 'https://site.test/watch/1')).not.toBe(pageResourceKey(10, 'https://site.test/watch/1'))
  })
  it('replays authentication and browser context without transport-owned headers', () => {
    expect(replayableRequestHeaders({
      Authorization: 'Bearer signed-token',
      'Sec-CH-UA': '"Chromium";v="140"',
      'X-Playback-Token': 'abc',
      Cookie: 'private=1',
      Host: 'cdn.test',
      Range: 'bytes=0-1',
      'Accept-Encoding': 'gzip, br',
    })).toEqual({
      authorization: 'Bearer signed-token',
      'sec-ch-ua': '"Chromium";v="140"',
      'x-playback-token': 'abc',
    })
  })
  it('never invents an Origin that the browser did not send', () => {
    expect(resourceRequestIdentity({
      pageUrl: 'https://page.test/watch/1',
      requestHeaders: { Referer: 'https://page.test/watch/1', 'User-Agent': 'Browser UA' },
    }, 'Fallback UA')).toEqual({
      referer: 'https://page.test/watch/1',
      origin: '',
      userAgent: 'Browser UA',
    })
    expect(resourceRequestIdentity({
      pageUrl: 'https://page.test/watch/1',
      requestHeaders: { Origin: 'https://page.test' },
    }, 'Fallback UA')).toEqual({
      referer: 'https://page.test/watch/1',
      origin: 'https://page.test',
      userAgent: 'Fallback UA',
    })
  })
  it('requires and matches a recent explicit click', () => {
    const base = { url: 'https://cdn.test/file.zip', size: 2048, enabled: true, minimumBytes: 1024, excludedHosts: [] }
    expect(shouldTakeover(base)).toBe(false)
    expect(shouldTakeover({ ...base, explicitClick: true })).toBe(true)
    expect(shouldTakeover({
      ...base,
      url: 'https://cdn.test/get?id=unknown',
      filename: '',
      explicitClick: true,
    })).toBe(true)
    const intent = {
      href: 'https://cdn.test/start',
      pageUrl: 'https://site.test/download#button',
      altBypass: false,
      ctrlForce: false,
      at: 1000,
    }
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/start',
      finalUrl: 'https://cdn.test/final.zip',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/start',
      referrer: 'https://other.test/download',
    }, 2000)).toBe(false)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/final.zip',
      finalUrl: 'https://cdn.test/mirror.zip',
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, tabId: 8 }, {
      url: 'https://cdn.test/final.zip',
      finalUrl: 'https://cdn.test/mirror.zip',
      chainUrls: ['https://cdn.test/start', 'https://cdn.test/final.zip'],
      referrer: 'https://github.test/redirected-download',
      tabId: 8,
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 8,
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 9,
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, tabId: 8, opensNewTab: true }, {
      url: 'https://cdn.test/start',
      finalUrl: 'https://cdn.test/file.zip',
      tabId: 9,
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true, controlHint: true, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 8,
    }, 4500)).toBe(true)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/start',
    }, 9000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: '' }, {
      url: 'https://cdn.test/advert.php',
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true, tabId: 8 }, {
      url: 'https://cdn.test/unrelated.zip',
      tabId: 8,
    }, 1800)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 2101)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: 'https://site.test/download#', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 3000)).toBe(false)
  })
})
