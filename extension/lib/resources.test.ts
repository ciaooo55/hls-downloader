import { describe, expect, it } from 'vitest'
import {
  canonicalMediaUrl,
  classifyDownload,
  looksLikeDownloadFile,
  isConcreteDownloadMime,
  classifyPlaybackSource,
  classifyResource,
  isSameDocumentPlaybackFallback,
  compactResources,
  isGenericMediaName,
  isUsefulResource,
  matchesDownloadClick,
  mergeResources,
  pageResourceKey,
  replayableRequestHeaders,
  resourceFingerprint,
  resourceId,
  playerPlaybackResources,
  resourceMatchesPlaybackSource,
  capturedRequestIdentity,
  resourceRequestIdentity,
  visiblePlaybackResources,
  shouldTakeover,
  suggestedResourceFilename,
  visibleMediaResources,
  normalizeHost,
  pruneExpiredResources,
  RESOURCE_CACHE_RETENTION_MS,
  resourceBelongsToFrame,
  boundedConfidence,
  isShortLivedMediaSignatureUsable,
  usesShortLivedMediaSignature,
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
  it('bounds non-finite recognition confidence', () => {
    expect(boundedConfidence(Number.NaN, 0.58)).toBe(0.58)
    expect(boundedConfidence(Number.POSITIVE_INFINITY)).toBe(0)
    expect(boundedConfidence(1.7)).toBe(1)
  })

  it('filters HLS segments but retains manifests', () => {
    expect(classifyResource('https://cdn.test/a.m3u8')).toBe('hls')
    expect(classifyResource('https://cdn.test/0001.ts')).toBeNull()
    expect(classifyResource('https://cdn.test/file.torrent?token=1')).toBe('file')
    expect(classifyResource('https://cdn.test/get?id=1', 'application/x-bittorrent')).toBe('file')
    expect(classifyResource('https://mirror.test/pkg.meta4')).toBe('file')
    expect(classifyResource('https://mirror.test/pkg.metalink', 'application/metalink4+xml')).toBe('file')
    expect(classifyResource('https://rr1.googlevideo.test/videoplayback?expire=1&mime=video%2Fmp4&itag=18')).toBe('media')
    expect(classifyResource('https://rr1.googlevideo.test/videoplayback?id=1', 'application/octet-stream')).toBe('media')
    expect(classifyResource('https://api.bilibili.test/x/player/playurl?cid=1', 'application/json')).toBeNull()
    expect(classifyResource('https://api.bilibili.test/x/player/playurl?cid=1&mime=video%2Fmp4', 'application/json')).toBeNull()
    expect(classifyResource('https://cdn.test/playlist.m3u')).toBe('hls')
    expect(classifyResource('https://cdn.test/poster.jpg', 'image/jpeg')).toBeNull()
  })
  it('uses an actually playing media element as direct classification evidence', () => {
    expect(classifyPlaybackSource('https://cdn.test/movie.mp4')).toBe('media')
    expect(classifyPlaybackSource('https://cdn.test/play?id=42')).toBe('media')
    expect(classifyPlaybackSource('https://cdn.test/master.m3u8')).toBe('hls')
    expect(classifyPlaybackSource('https://cn.pornhub.com/view_video.php?viewkey=123')).toBeNull()
    expect(classifyPlaybackSource('https://cdn.test/player.php?id=42', 'video/mp4')).toBe('media')
    expect(classifyPlaybackSource('https://rr1.googlevideo.test/videoplayback?expire=1&mime=video%2Fmp4&itag=18')).toBe('media')
    expect(classifyPlaybackSource('blob:https://site.test/opaque')).toBeNull()
    expect(classifyPlaybackSource('https://cdn.test/poster.jpg')).toBeNull()
    expect(classifyPlaybackSource('https://cdn.test/player.js')).toBeNull()
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
  it('expires old media observations without requiring the popup to open', () => {
    const now = 1_800_000_000_000
    const current = resource({ id: 'current', seenAt: now - RESOURCE_CACHE_RETENTION_MS + 1 })
    const stale = resource({ id: 'stale', url: 'https://cdn.test/stale.mp4', seenAt: now - RESOURCE_CACHE_RETENTION_MS })
    expect(pruneExpiredResources([current, stale], now).map(item => item.id)).toEqual(['current'])
  })
  it('keeps iframe resource views isolated while allowing untagged top-frame evidence', () => {
    expect(resourceBelongsToFrame({ frameId: 2 }, 2)).toBe(true)
    expect(resourceBelongsToFrame({ frameId: 1 }, 2)).toBe(false)
    expect(resourceBelongsToFrame({}, 0)).toBe(true)
    expect(resourceBelongsToFrame({ frameId: 3 }, 0)).toBe(false)
    expect(resourceBelongsToFrame({ frameId: 2 }, -1)).toBe(true)
  })
  it('can retain identical media URLs from distinct frames in the shared cache', () => {
    const first = resource({ frameId: 1, seenAt: Date.now() - 2 })
    const second = resource({ frameId: 2, seenAt: Date.now() })
    expect(compactResources([first, second], 40)).toHaveLength(1)
    expect(compactResources([first, second], 40, true)).toHaveLength(2)
  })
  it('filters explicit advert query signals without rejecting ordinary words or disabled flags', () => {
    expect(isUsefulResource(resource({ url: 'https://cdn.test/play.mp4?ad=preroll' }))).toBe(false)
    expect(isUsefulResource(resource({ url: 'https://cdn.test/play.mp4?role=midroll' }))).toBe(false)
    expect(isUsefulResource(resource({ url: 'https://cdn.test/play.mp4?ad=0&topic=adventure' }))).toBe(true)
    expect(isUsefulResource(resource({ url: 'https://cdn.test/adventure/play.mp4' }))).toBe(true)
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

    const shortLivedPrevious = resource({
      kind: 'media',
      url: 'https://cdn.test/clip.mp4?quality=1080&s=old-signature&e=100&_t=90',
    })
    const shortLivedRefreshed = resource({
      kind: 'media',
      url: 'https://cdn.test/clip.mp4?_t=190&e=200&s=new-signature&quality=1080',
    })
    expect(resourceFingerprint(shortLivedPrevious)).toBe(resourceFingerprint(shortLivedRefreshed))
    expect(resourceFingerprint(shortLivedPrevious)).not.toBe(resourceFingerprint({
      ...shortLivedRefreshed,
      url: 'https://cdn.test/clip.mp4?s=new-signature&e=200&quality=720',
    }))
    expect(resourceFingerprint(resource({
      kind: 'file', url: 'https://api.test/export?token=customer-a',
    }))).not.toBe(resourceFingerprint(resource({
      kind: 'file', url: 'https://api.test/export?token=customer-b',
    })))

    const directFileOld = resource({
      kind: 'file', url: 'https://mxcontent.test/v2/asset.mp4?s=old&e=1786200000&_t=1786180000&quality=1080',
    })
    const directFileNew = resource({
      kind: 'file', url: 'https://mxcontent.test/v2/asset.mp4?s=new&e=1786300000&_t=1786190000&quality=1080',
    })
    expect(usesShortLivedMediaSignature(directFileOld)).toBe(true)
    expect(resourceFingerprint(directFileOld)).toBe(resourceFingerprint(directFileNew))
    expect(isShortLivedMediaSignatureUsable(directFileNew, 1_786_290_000_000)).toBe(true)
    expect(isShortLivedMediaSignatureUsable(directFileOld, 1_786_290_000_000)).toBe(false)
    expect(usesShortLivedMediaSignature(resource({
      kind: 'file', url: 'https://api.test/export?s=customer-a&e=invoice-42',
    }))).toBe(false)
  })

  it('canonicalizes LL-HLS cursors without re-encoding signed query bytes', () => {
    expect(canonicalMediaUrl(
      'https://edge.test/live.m3u8?token=a%2Fb%2Bc&_HLS_msn=4&_HLS_part=2&empty=',
      'hls',
    )).toBe('https://edge.test/live.m3u8?token=a%2Fb%2Bc&empty=')
  })
  it('matches the current player across refreshed signatures without merging distinct renditions', () => {
    const captured = resource({
      kind: 'hls',
      url: 'https://cdn.test/master.m3u8?quality=1080&token=renewed&expires=900',
    })

    expect(resourceMatchesPlaybackSource(captured, 'https://cdn.test/master.m3u8?expires=100&token=old&quality=1080')).toBe(true)
    expect(resourceMatchesPlaybackSource(captured, 'https://cdn.test/master.m3u8?quality=720&token=renewed')).toBe(false)
    expect(resourceMatchesPlaybackSource(captured, 'blob:https://site.test/current-player')).toBe(false)
  })
  it('folds captured HLS variants into their master manifest', () => {
    const now = Date.now()
    const master = resource({
      id: 'master',
      kind: 'hls',
      url: 'https://cdn.test/master.m3u8?token=master',
      seenAt: now - 20_000,
      variants: [
        { url: 'https://cdn.test/1080p/index.m3u8?token=from-manifest', height: 1080 },
        { url: 'https://cdn.test/720p/index.m3u8?token=from-manifest', height: 720 },
      ],
    })
    const high = resource({
      id: '1080p-child',
      kind: 'hls',
      url: 'https://cdn.test/1080p/index.m3u8?token=observed',
      seenAt: now - 1_000,
    })
    const medium = resource({
      id: '720p-child',
      kind: 'hls',
      url: 'https://cdn.test/720p/index.m3u8?token=refreshed',
      seenAt: now,
    })

    expect(compactResources([high, master, medium])).toMatchObject([{
      id: 'master',
      url: master.url,
      seenAt: now,
    }])
  })
  it('folds a generic external audio playlist into its owning HLS master', () => {
    const now = Date.now()
    const master = resource({
      id: 'master', kind: 'hls', url: 'https://cdn.test/session/master.m3u8', seenAt: now - 1_000,
      variants: [{ url: 'https://cdn.test/session/video.m3u8', height: 1080 }],
      renditionUrls: ['https://cdn.test/session/track.m3u8?token=old'],
    })
    const audio = resource({
      id: 'audio', kind: 'hls', url: 'https://cdn.test/session/track.m3u8?token=new',
      playbackUrls: ['https://cdn.test/session/audio-10.m4s'], seenAt: now,
    })

    expect(compactResources([audio, master])).toMatchObject([{
      id: 'master',
      playbackUrls: ['https://cdn.test/session/audio-10.m4s'],
      seenAt: now,
    }])
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
  it('uses an exact current-video source without mixing in nearby streams', () => {
    const now = Date.now()
    const direct = resource({ id: 'direct', kind: 'media', url: 'https://cdn.test/current.mp4', size: 12 * 1024 * 1024, seenAt: now - 60_000 })
    const main = resource({ id: 'main', kind: 'hls', url: 'https://cdn.test/main.m3u8', duration: 3_600, bandwidth: 2_000_000, seenAt: now })
    const bumper = resource({ id: 'bumper', kind: 'hls', url: 'https://cdn.test/bumper.m3u8', duration: 15, bandwidth: 6_000_000, seenAt: now + 1 })
    const stale = resource({ id: 'stale', kind: 'hls', url: 'https://cdn.test/stale.m3u8', size: 2_000_000_000, seenAt: now - 60_000 })

    expect(visiblePlaybackResources([direct, main, bumper, stale], null)).toEqual([])
    expect(visiblePlaybackResources([direct, main, bumper, stale], { sourceUrls: [direct.url], startedAt: now }).map(item => item.id)).toEqual(['direct'])
  })
  it('canonicalizes LL-HLS poll cursors into one reusable stream URL', () => {
    const first = resource({
      id: 'poll-1', kind: 'hls', seenAt: 100,
      url: 'https://edge.test/live.m3u8?_HLS_msn=100&_HLS_part=2&session=current',
    })
    const second = resource({
      id: 'poll-2', kind: 'hls', seenAt: 200,
      url: 'https://edge.test/live.m3u8?session=current&_HLS_msn=101&_HLS_part=0',
    })

    expect(canonicalMediaUrl(first.url, 'hls')).toBe('https://edge.test/live.m3u8?session=current')
    expect(resourceFingerprint(first)).toBe(resourceFingerprint(second))
    expect(compactResources([first, second])).toMatchObject([{
      url: 'https://edge.test/live.m3u8?session=current',
      seenAt: 200,
    }])
  })
  it('keeps a signed direct player URL at the exact-evidence tier after its token refreshes', () => {
    const now = Date.now()
    const refreshed = resource({
      id: 'refreshed-direct', kind: 'media', size: 20 * 1024 * 1024,
      url: 'https://cdn.test/movie.mp4?quality=1080&token=new&expires=900', seenAt: now - 30_000,
    })
    const recentFallback = resource({
      id: 'fallback', kind: 'hls', duration: 3600, bandwidth: 1_000_000,
      url: 'https://cdn.test/other/master.m3u8', seenAt: now,
    })

    expect(visiblePlaybackResources([recentFallback, refreshed], {
      sourceUrls: ['https://cdn.test/movie.mp4?expires=100&quality=1080&token=old'], startedAt: now,
    }).map(item => item.id)).toEqual(['refreshed-direct'])
  })
  it('keeps preloaded and late adaptive manifests for MSE playback, not small media responses', () => {
    const now = Date.now()
    const preloaded = resource({
      id: 'preloaded', kind: 'hls', url: 'https://cdn.test/master.m3u8', inspected: true, seenAt: now - 2 * 60_000,
    })
    const lateRendition = resource({
      id: 'late-rendition', kind: 'dash', url: 'https://cdn.test/manifest.mpd', inspected: true, seenAt: now + 5 * 60_000,
    })
    const smallMedia = resource({
      id: 'fragment', kind: 'media', url: 'https://cdn.test/opaque', size: 176 * 1024, seenAt: now,
    })

    expect(visiblePlaybackResources([smallMedia, lateRendition, preloaded], {
      sourceUrls: ['blob:https://site.test/current-player'], startedAt: now,
    }).map(item => item.id)).toEqual(['preloaded', 'late-rendition'])
  })
  it('uses SourceBuffer path evidence to separate simultaneous MSE players', () => {
    const now = Date.now()
    const first = resource({
      id: 'first', kind: 'hls', inspected: true,
      url: 'https://cdn.test/live/channel-one/master.m3u8', seenAt: now,
    })
    const second = resource({
      id: 'second', kind: 'hls', inspected: true,
      url: 'https://cdn.test/live/channel-two/master.m3u8', seenAt: now,
    })

    expect(playerPlaybackResources([first, second], {
      sourceUrls: ['blob:https://site.test/player-one'],
      mseResourceUrls: ['https://cdn.test/live/channel-one/segment-10.m4s'],
      startedAt: now,
    }, 2).map(item => item.id)).toEqual(['first'])
    expect(playerPlaybackResources([first, second], {
      sourceUrls: ['blob:https://site.test/player-two'],
      mseResourceUrls: ['https://cdn.test/live/channel-two/segment-20.m4s'],
      startedAt: now,
    }, 2).map(item => item.id)).toEqual(['second'])
  })
  it('binds YouTube-style videoplayback MSE bytes to the playing video', () => {
    const now = Date.now()
    const stream = 'https://rr1.googlevideo.test/videoplayback?expire=1&mime=video%2Fmp4&itag=18'
    const item = resource({
      id: 'yt', kind: 'media', url: stream, seenAt: now,
    })
    expect(playerPlaybackResources([item], {
      sourceUrls: ['blob:https://site.test/player'],
      mseResourceUrls: [stream],
      startedAt: now,
    }, 1).map(entry => entry.id)).toEqual(['yt'])
  })
  it('does not assign origin-only MSE evidence to either player', () => {
    const now = Date.now()
    const first = resource({
      id: 'first', kind: 'hls', inspected: true,
      url: 'https://cdn.test/manifests/one.m3u8', seenAt: now,
    })
    const second = resource({
      id: 'second', kind: 'hls', inspected: true,
      url: 'https://cdn.test/manifests/two.m3u8', seenAt: now,
    })
    expect(playerPlaybackResources([first, second], {
      sourceUrls: ['blob:https://site.test/player'],
      mseResourceUrls: ['https://cdn.test/segments/chunk.m4s'],
      startedAt: now,
    }, 2)).toEqual([])
  })
  it('uses parsed segment URLs when two manifests share one CDN directory', () => {
    const now = Date.now()
    const first = resource({
      id: 'first', kind: 'hls', inspected: true,
      url: 'https://cdn.test/live/one.m3u8',
      playbackUrls: ['https://cdn.test/live/shared-101.m4s?token=old'],
      seenAt: now,
    })
    const second = resource({
      id: 'second', kind: 'hls', inspected: true,
      url: 'https://cdn.test/live/two.m3u8',
      playbackUrls: ['https://cdn.test/live/shared-202.m4s?token=old'],
      seenAt: now,
    })

    expect(playerPlaybackResources([first, second], {
      sourceUrls: ['blob:https://site.test/player-two'],
      mseResourceUrls: ['https://cdn.test/live/shared-202.m4s?token=new'],
      startedAt: now,
    }, 2).map(item => item.id)).toEqual(['second'])
  })
  it('binds an exact MSE progressive MP4 even when the network classified it as a file', () => {
    const now = Date.now()
    const item = resource({
      id: 'movie', kind: 'file', url: 'https://cdn.test/movie.mp4?sig=old', seenAt: now,
    })
    expect(playerPlaybackResources([item], {
      sourceUrls: ['blob:https://site.test/player'],
      mseResourceUrls: ['https://cdn.test/movie.mp4?sig=new'],
      startedAt: now,
    }, 1).map(entry => entry.id)).toEqual(['movie'])
  })
  it('collapses file and media observations of the same MSE response', () => {
    const now = Date.now()
    const url = 'https://cdn.test/movie.mp4?channel=single'
    const file = resource({ id: 'network-file', kind: 'file', url, seenAt: now })
    const media = resource({ id: 'source-buffer-media', kind: 'media', url, seenAt: now + 1 })

    expect(playerPlaybackResources([file, media], {
      sourceUrls: ['blob:https://site.test/player'],
      mseResourceUrls: [url],
      startedAt: now,
    }, 1).map(entry => entry.id)).toEqual(['source-buffer-media'])
  })
  it('does not bind a same-folder file from weak MSE path affinity', () => {
    const now = Date.now()
    const preview = resource({
      id: 'preview', kind: 'file', url: 'https://cdn.test/vod/preview.mp4', seenAt: now,
    })
    expect(playerPlaybackResources([preview], {
      sourceUrls: ['blob:https://site.test/player'],
      mseResourceUrls: ['https://cdn.test/vod/segment-1.m4s'],
      startedAt: now,
    }, 1)).toEqual([])
  })
  it('does not treat a watch page URL as the playing media file', () => {
    expect(isSameDocumentPlaybackFallback('https://site.test/watch?v=1', 'https://site.test/watch?v=1')).toBe(true)
    expect(isSameDocumentPlaybackFallback('https://site.test/watch?v=1#player', 'https://site.test/watch?v=1')).toBe(true)
    expect(isSameDocumentPlaybackFallback('https://site.test/movie.mp4', 'https://site.test/movie.mp4')).toBe(false)
    expect(isSameDocumentPlaybackFallback('https://cdn.test/play?id=42', 'https://site.test/watch')).toBe(false)
  })
  it('associates a response Blob URL with its exact direct media resource', () => {
    const now = Date.now()
    const direct = resource({
      id: 'direct', kind: 'media', url: 'https://cdn.test/movie.mp4?sig=old', seenAt: now,
    })

    expect(playerPlaybackResources([direct], {
      sourceUrls: ['blob:https://site.test/movie'],
      mseResourceUrls: ['https://cdn.test/movie.mp4?sig=new'],
      startedAt: now,
    }, 1).map(item => item.id)).toEqual(['direct'])
  })
  it('does not erase semantic query ids when assigning one MSE player', () => {
    const now = Date.now()
    const first = resource({ id: 'first', kind: 'media', url: 'https://cdn.test/video.mp4?id=one', seenAt: now })
    const second = resource({ id: 'second', kind: 'media', url: 'https://cdn.test/video.mp4?id=two', seenAt: now })

    expect(playerPlaybackResources([first, second], {
      sourceUrls: ['blob:https://site.test/two'],
      mseResourceUrls: ['https://cdn.test/video.mp4?id=two'],
      startedAt: now,
    }, 2).map(item => item.id)).toEqual(['second'])
  })
  it('matches DASH SegmentTemplate wildcards without confusing representations', () => {
    const now = Date.now()
    const first = resource({
      id: 'first', kind: 'dash', url: 'https://cdn.test/manifest.mpd?camera=one',
      playbackPatterns: ['https://cdn.test/chunk-v1-*.m4s?camera=one'], seenAt: now,
    })
    const second = resource({
      id: 'second', kind: 'dash', url: 'https://cdn.test/manifest.mpd?camera=two',
      playbackPatterns: ['https://cdn.test/chunk-v2-*.m4s?camera=two'], seenAt: now,
    })

    expect(playerPlaybackResources([first, second], {
      sourceUrls: ['blob:https://site.test/dash-two'],
      mseResourceUrls: ['https://cdn.test/chunk-v2-00042.m4s?camera=two&token=rotated'],
      startedAt: now,
    }, 2).map(item => item.id)).toEqual(['second'])
  })
  it('waits for inspection when multiple raw MSE manifests could include an advert', () => {
    const now = Date.now()
    const advert = resource({
      id: 'advert', kind: 'hls', url: 'https://media.test/opening.m3u8', seenAt: now,
    })
    const main = resource({
      id: 'main', kind: 'hls', url: 'https://media.test/feature.m3u8', seenAt: now + 100,
    })

    expect(visiblePlaybackResources([advert, main], {
      sourceUrls: ['blob:https://site.test/player'], startedAt: now,
    })).toEqual([])
  })
  it('keeps a verified live stream and removes a pre-roll from the same MSE session', () => {
    const now = Date.now()
    const preroll = resource({
      id: 'preroll', kind: 'hls', url: 'https://media.test/opening.m3u8',
      inspected: true, isLive: false, duration: 20, bandwidth: 8_000_000,
      seenAt: now - 2_000,
    })
    const live = resource({
      id: 'live', kind: 'hls', url: 'https://edge.test/streams/channel/llhls.m3u8',
      inspected: true, isLive: true, lowLatencyLive: true, bandwidth: 3_000_000,
      seenAt: now + 500,
    })

    expect(visiblePlaybackResources([preroll, live], {
      sourceUrls: ['blob:https://site.test/player'], startedAt: now,
    }).map(item => item.id)).toEqual(['live'])
  })
  it('removes a verified short bumper when a long VOD is available', () => {
    const now = Date.now()
    const bumper = resource({
      id: 'bumper', kind: 'hls', url: 'https://media.test/opening.m3u8',
      inspected: true, isLive: false, duration: 12, seenAt: now,
    })
    const movie = resource({
      id: 'movie', kind: 'hls', url: 'https://media.test/feature.m3u8',
      inspected: true, isLive: false, duration: 3_600, seenAt: now + 100,
    })

    expect(visiblePlaybackResources([bumper, movie], {
      sourceUrls: ['blob:https://site.test/player'], startedAt: now,
    }).map(item => item.id)).toEqual(['movie'])
  })
  it('keeps both verified live routes and prefers completed segments', () => {
    const now = Date.now()
    const event = resource({
      id: 'event', kind: 'hls', url: 'https://media.test/event.m3u8',
      inspected: true, isLive: true, bandwidth: 9_000_000, seenAt: now + 100,
    })
    const lowLatency = resource({
      id: 'llhls', kind: 'hls', url: 'https://edge.test/streams/channel/llhls.m3u8',
      inspected: true, isLive: true, lowLatencyLive: true, partOnlyLive: true,
      bandwidth: 3_000_000, seenAt: now,
    })

    expect(visiblePlaybackResources([event, lowLatency], {
      sourceUrls: ['blob:https://site.test/player'], startedAt: now,
    }).map(item => item.id)).toEqual(['event', 'llhls'])
  })
  it('does not hide the only verified short-form stream', () => {
    const now = Date.now()
    const clip = resource({
      id: 'clip', kind: 'hls', url: 'https://media.test/clip.m3u8',
      inspected: true, isLive: false, duration: 20, seenAt: now,
    })

    expect(visiblePlaybackResources([clip], {
      sourceUrls: ['blob:https://site.test/player'], startedAt: now,
    }).map(item => item.id)).toEqual(['clip'])
  })
  it('does not revive adaptive manifests from an earlier page session', () => {
    const now = Date.now()
    const earlierPageVideo = resource({
      id: 'earlier', kind: 'hls', url: 'https://cdn.test/earlier.m3u8', seenAt: now - 3 * 60_000 - 1,
    })

    expect(visiblePlaybackResources([earlierPageVideo], {
      sourceUrls: ['blob:https://site.test/current-player'], startedAt: now,
    })).toEqual([])
  })
  it('does not fall back to unrelated page media when playback has no evidence', () => {
    const now = Date.now()
    const unrelated = resource({
      id: 'unrelated', kind: 'hls', url: 'https://ads.test/background/master.m3u8',
      duration: 3_600, seenAt: now - 5 * 60_000,
    })

    expect(visiblePlaybackResources([unrelated], { sourceUrls: ['blob:https://site.test/player'], startedAt: now })).toEqual([])
  })
  it('hides ambiguous adaptive manifests when simultaneous MSE players are active', () => {
    const now = Date.now()
    const manifest = resource({
      id: 'manifest', kind: 'hls', url: 'https://cdn.test/current/master.m3u8', seenAt: now,
    })
    const blobPlayback = { sourceUrls: ['blob:https://site.test/player-a'], startedAt: now }

    expect(playerPlaybackResources([manifest], blobPlayback, 1).map(item => item.id)).toEqual(['manifest'])
    expect(playerPlaybackResources([manifest], blobPlayback, 2)).toEqual([])
  })
  it('never places a recent adaptive stream beside a different direct video', () => {
    const now = Date.now()
    const manifest = resource({
      id: 'other-player', kind: 'hls', url: 'https://cdn.test/other/master.m3u8', seenAt: now,
    })
    const directPlayback = { sourceUrls: ['https://cdn.test/current/movie.mp4'], startedAt: now }

    expect(playerPlaybackResources([manifest], directPlayback, 0)).toEqual([])
  })
  it('keeps images and ambiguous dynamic documents in the browser', () => {
    expect(classifyDownload('https://cdn.test/photo.jpg', 'application/octet-stream', 'photo.jpg')).toBeNull()
    expect(classifyDownload('https://site.test/advert.php', 'application/octet-stream')).toBeNull()
    expect(classifyDownload('https://site.test/export.php', 'application/pdf', 'report.pdf')).toBe('file')
    expect(classifyDownload(
      'https://cdn.test/get?id=1',
      'application/octet-stream',
      'download',
      'attachment; filename="download"',
    )).toBe('file')
    expect(classifyDownload(
      'https://site.test/export.php',
      'application/octet-stream',
      '',
      'attachment',
    )).toBe('file')
    expect(classifyDownload('https://cdn.test/get?id=1', 'application/octet-stream', 'download')).toBeNull()
  })
  it('takes over ordinary files whose URL already has a download extension', () => {
    expect(looksLikeDownloadFile('https://mirror.test/ubuntu-24.04.iso?token=1')).toBe(true)
    expect(looksLikeDownloadFile('https://site.test/export.csv')).toBe(true)
    expect(looksLikeDownloadFile('https://site.test/export.php?file=ubuntu.iso')).toBe(false)
    expect(classifyDownload('https://cdn.test/ubuntu-24.04.iso', 'application/octet-stream', 'download')).toBe('file')
    expect(classifyDownload('https://cdn.test/ubuntu-24.04.iso', '', '')).toBe('file')
    expect(classifyDownload('https://cdn.test/app-1.2.3.apk', 'application/octet-stream', '')).toBe('file')
    expect(classifyDownload('https://cdn.test/archive.tar.gz', 'application/octet-stream', '')).toBe('file')
    expect(classifyDownload('https://cdn.test/Setup.dmg', 'application/octet-stream', 'download')).toBe('file')
    expect(classifyDownload('https://cdn.test/report.docx', 'application/octet-stream', '')).toBe('file')
    expect(classifyDownload('https://cdn.test/legacy.f4v', '', '')).toBe('file')
    expect(classifyDownload('https://cdn.test/clip.3gp', '', '')).toBe('file')
    expect(looksLikeDownloadFile('https://cdn.test/song.aac')).toBe(true)
    expect(classifyDownload('https://mirror.test/pkg.meta4', 'application/metalink4+xml', 'pkg.meta4')).toBe('file')
    expect(classifyDownload('https://mirror.test/pkg.metalink', '', 'pkg.metalink')).toBe('file')
    expect(isConcreteDownloadMime('application/metalink4+xml')).toBe(true)
    expect(classifyDownload('https://site.test/advert.php', 'application/octet-stream')).toBeNull()
    expect(classifyDownload('https://cdn.test/get?id=1', 'application/octet-stream', 'download')).toBeNull()
  })
  it('takes over PHP/ASP downloads when the server names a concrete file MIME', () => {
    expect(isConcreteDownloadMime('application/zip; charset=binary')).toBe(true)
    expect(isConcreteDownloadMime('application/pdf')).toBe(true)
    expect(isConcreteDownloadMime('application/octet-stream')).toBe(false)
    expect(classifyDownload('https://site.test/export.php', 'application/zip', 'export.php')).toBe('file')
    expect(classifyDownload('https://site.test/get.aspx', 'application/pdf', 'get.aspx')).toBe('file')
    expect(classifyDownload('https://site.test/download.php', 'application/force-download', '')).toBe('file')
    expect(classifyDownload('https://site.test/advert.php', 'application/octet-stream')).toBeNull()
    expect(classifyDownload('https://site.test/export.php', 'application/javascript', 'export.php')).toBeNull()
  })
  it('excludes passive web resources unless the server marks an attachment', () => {
    expect(classifyDownload('https://site.test/app.js', 'application/javascript', 'app.js')).toBeNull()
    expect(classifyDownload('https://site.test/style.css', 'text/css', 'style.css')).toBeNull()
    expect(classifyDownload('https://site.test/data', 'application/json', 'data.json')).toBeNull()
    expect(classifyDownload('https://site.test/font.woff2', 'font/woff2', 'font.woff2')).toBeNull()
    expect(classifyDownload(
      'https://site.test/export',
      'application/json',
      'report.json',
      'attachment; filename="report.json"',
    )).toBe('file')
  })
  it('honors Alt bypass and Ctrl force', () => {
    const base = { url: 'https://a.test/file.zip', size: 20, enabled: true, minimumBytes: 10, excludedHosts: [], explicitClick: true }
    expect(shouldTakeover({ ...base, altBypass: true })).toBe(false)
    expect(shouldTakeover({ ...base, enabled: false, ctrlForce: true })).toBe(true)
    expect(shouldTakeover({ ...base, size: 9 })).toBe(false)
    expect(shouldTakeover({ ...base, size: 0 })).toBe(true)
    expect(shouldTakeover({ ...base, url: 'https://sub.blocked.test/file.zip', excludedHosts: ['blocked.test'] })).toBe(false)
    expect(shouldTakeover({
      url: 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567',
      size: 0, enabled: true, minimumBytes: 10, excludedHosts: [], explicitClick: true,
    })).toBe(true)
    expect(shouldTakeover({
      url: 'ftp://files.test/a.bin',
      size: 20, enabled: true, minimumBytes: 0, excludedHosts: [], explicitClick: true,
    })).toBe(false)
  })
  it('applies an excluded source page to CDN downloads and normalizes ports', () => {
    expect(normalizeHost('https://WWW.Example.test:443/watch')).toBe('example.test')
    expect(shouldTakeover({
      url: 'https://media.cdn.test/file.mp4', sourcePageUrl: 'https://www.Example.test:443/watch',
      size: 20, enabled: true, minimumBytes: 0, excludedHosts: ['example.test'], explicitClick: true,
    })).toBe(false)
  })
  it('never takes over OAuth/account navigation, including forced or stale click paths', () => {
    const base = { size: 20, enabled: true, minimumBytes: 10, excludedHosts: [], explicitClick: true, ctrlForce: true }
    const google = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=app&redirect_uri=https%3A%2F%2Fsite.test%2Fcallback&response_type=code'
    const microsoft = 'https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=app&redirect_uri=https%3A%2F%2Fsite.test%2Fcallback&response_type=code'
    const github = 'https://github.com/login/oauth/authorize?client_id=app&redirect_uri=https%3A%2F%2Fsite.test%2Fcallback'
    expect(shouldTakeover({ ...base, url: google })).toBe(false)
    expect(shouldTakeover({ ...base, url: microsoft })).toBe(false)
    expect(shouldTakeover({ ...base, url: github })).toBe(false)
    expect(matchesDownloadClick({
      href: google, pageUrl: 'https://site.test/login', altBypass: false, ctrlForce: false, at: 1_000,
    }, { url: 'https://cdn.test/file.zip', finalUrl: 'https://cdn.test/file.zip' }, 1_500)).toBe(false)
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
  it('uses the browser page URL as media Referer and Origin', () => {
    expect(resourceRequestIdentity({
      pageUrl: 'https://page.test/watch/1',
      requestHeaders: { Referer: 'https://page.test/watch/1', 'User-Agent': 'Browser UA' },
    }, 'Fallback UA')).toEqual({
      referer: 'https://page.test/watch/1',
      origin: 'https://page.test',
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
    expect(resourceRequestIdentity({
      pageUrl: 'https://page.test/watch/1',
      requestHeaders: {
        Referer: 'https://cdn.test/video.m3u8',
        Origin: 'https://cdn.test',
      },
    })).toMatchObject({
      referer: 'https://page.test/watch/1',
      origin: 'https://page.test',
    })
  })
  it('uses stable 128-bit resource identifiers', () => {
    const first = resourceId('https://cdn.test/video.mp4?quality=1080')
    const second = resourceId('https://cdn.test/video.mp4?quality=720')
    expect(first).toMatch(/^[0-9a-f]{32}$/)
    expect(first).toBe(resourceId('https://cdn.test/video.mp4?quality=1080'))
    expect(first).not.toBe(second)
  })
  it('keeps the exact per-origin identity and does not invent an Origin header', () => {
    expect(capturedRequestIdentity({
      Referer: 'https://embed.test/player',
      'User-Agent': 'Browser UA',
    }, 'Fallback UA')).toEqual({
      referer: 'https://embed.test/player',
      origin: '',
      userAgent: 'Browser UA',
    })
  })
  it('uses a recent click as confidence but accepts a classified DownloadItem as strong evidence', () => {
    const base = { url: 'https://cdn.test/file.zip', size: 2048, enabled: true, minimumBytes: 1024, excludedHosts: [] }
    expect(shouldTakeover(base)).toBe(false)
    expect(shouldTakeover({ ...base, strongEvidence: true })).toBe(true)
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
    const downloadControl = { ...intent, controlHint: true }
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
    // Gateway/JS downloads often report a final CDN URL that differs from the
    // clicked href. Same-tab + same-page (or missing Chrome referrer) still
    // counts as the user's click; cross-tab must stay rejected.
    expect(matchesDownloadClick({ ...downloadControl, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 8,
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...downloadControl, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 9,
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...downloadControl, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      tabId: 8,
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...downloadControl, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      tabId: 8,
    }, 4000)).toBe(false)
    // A play-page href without a download control must not claim a later zip.
    expect(matchesDownloadClick({
      href: 'https://site.test/watch/episode-1?download=0',
      pageUrl: 'https://site.test/watch/episode-1?download=0',
      altBypass: false, ctrlForce: false, at: 1000, tabId: 8,
    }, {
      url: 'https://cdn.test/generated.zip',
      tabId: 8,
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
    }, 2101)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 3600)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: 'https://site.test/download#', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 3000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 4000)).toBe(false)
  })
})
