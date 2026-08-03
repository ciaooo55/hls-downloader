import { isAuthenticationNavigation } from './clickIntent'
import { httpOrigin } from './requestChain'
import { removeRawQueryParameters } from './urlQuery'

export type ResourceKind = 'hls' | 'dash' | 'media' | 'file' | 'magnet'

export interface MediaVariant {
  url: string
  width?: number
  height?: number
  bandwidth?: number
  quality?: string
}

export interface MediaResource {
  id: string
  url: string
  kind: ResourceKind
  mimeType?: string
  size?: number
  /** Estimated bytes for an adaptive stream when a manifest exposes duration + bitrate. */
  estimatedSize?: number
  pageUrl?: string
  title?: string
  filename?: string
  tabId?: number
  frameId?: number
  statusCode?: number
  method?: string
  requestHeaders?: Record<string, string>
  width?: number
  height?: number
  bandwidth?: number
  quality?: string
  duration?: number
  /** True/false only after an adaptive media playlist was actually parsed. */
  isLive?: boolean
  /** Strong signal for LL-HLS (PART/SERVER-CONTROL) rather than a short event. */
  lowLatencyLive?: boolean
  /** The inspected live window currently exposes only LL-HLS partial segments. */
  partOnlyLive?: boolean
  /** Distinguishes verified manifest metadata from URL/MIME-only detection. */
  inspected?: boolean
  manifestType?: 'master' | 'media'
  variants?: MediaVariant[]
  seenAt: number
}

export interface DownloadClickIntent {
  href: string
  pageUrl: string
  altBypass: boolean
  ctrlForce: boolean
  at: number
  generic?: boolean
  tabId?: number
  frameId?: number
  opensNewTab?: boolean
  controlHint?: boolean
}

const MEDIA_EXT = /\.(m3u8|mpd|mp4|webm|mkv|mov|avi|m4a|mp3|flac|wav|torrent|zip|7z|rar|exe|msi|pdf)(?:$|[?#])/i
const SEGMENT_EXT = /\.(?:ts|m4s|cmfv|cmfa|aac)(?:$|[?#])/i
const IMAGE_EXT = /\.(?:avif|bmp|gif|ico|jpe?g|png|svg|webp)(?:$|[?#])/i
const PASSIVE_WEB_EXT = /\.(?:cjs|css|eot|js|json|map|mjs|otf|ttf|wasm|woff2?|xml)(?:$|[?#])/i
const PASSIVE_WEB_MIME = /^(?:application\/(?:ecmascript|javascript|json|ld\+json|manifest\+json|wasm|xml)|font\/|text\/(?:css|javascript|xml))\b/i
const DYNAMIC_DOCUMENT_EXT = /\.(?:asp|aspx|cfm|cgi|do|action|jsp|php\d?)(?:$|[?#])/i
const MANIFEST_EXT = /\.(?:m3u8?|mpd)$/i
const SEGMENT_PATH = /(?:^|[\/_-])(?:init|segment|seg|fragment|frag|chunk|part)[-_]?(?:\d{1,8}|video|audio)?(?:\.|[\/_-]|$)/i
const SEGMENT_MIME = /^(?:video\/mp2t|audio\/(?:aac|mp4a-latm))\b/i
const VOLATILE_QUERY = /^(?:signature|sig|expires?|expiry|policy|key-pair-id|hdnea|hmac|access[_-]?key|x-amz-.+)$/i
const MEDIA_AUTH_QUERY = /^(?:token|auth|authorization|jwt|session|sessionid)$/i
const LL_HLS_RELOAD_QUERY = new Set(['_hls_msn', '_hls_part', '_hls_skip'])
const AD_SIGNAL = /(?:^|[\/_-])(?:ad|ads|advert|advertisement|preroll|midroll|postroll|promo)(?:[\/_-]|$)/i
const NON_VIDEO_MANIFEST_SIGNAL = /(?:^|[\/_.-])(?:audio(?:only|track)?|subtitle(?:s)?|caption(?:s)?|thumbnail(?:s)?|thumb(?:s)?|sprite(?:s)?|storyboard(?:s)?|preview(?:s)?|iframe|trickplay|ad(?:s)?|advert(?:s|ising)?|preroll|midroll|postroll)(?:[\/_.-]|$)/i
const NON_VIDEO_MANIFEST_QUERY = new Set(['audio', 'subtitle', 'subtitles', 'caption', 'captions', 'ad', 'ads', 'advertisement', 'iframe'])
const GENERIC_MEDIA_NAME = /^(?:(?:video|stream|master|index|playlist|manifest|chunklist|media|output|download|file|vod|live)(?:[-_ ]*(?:\d{3,4}p?|low|medium|high|sd|hd|fhd|uhd|4k))?|(?:hls[-_ ]*)?(?:\d{3,4}p[-_ ]*)?(?:hls[-_ ]*)?(?:video[-_ ]*stream|视频流)?|(?:hls[-_ ]*)?\d{3,4}p)$/i
const OPAQUE_MEDIA_NAME = /^(?:[a-f0-9]{16,}|[a-z0-9_-]{28,})$/i

function cleanName(value = '', pathValue = false): string {
  let result = value.trim()
  try { result = decodeURIComponent(result) } catch {}
  if (pathValue) result = result.replace(/\\/g, '/').split('/').pop() || ''
  result = result.split(/[?#]/, 1)[0].replace(MANIFEST_EXT, '').replace(/[<>:"/\\|?*]/g, '_').replace(/\s+/g, ' ').trim().replace(/^[. ]+|[. ]+$/g, '')
  return result.slice(0, 200)
}

export function isGenericMediaName(value = ''): boolean {
  const name = cleanName(value, true)
  if (!name) return true
  const stem = name.includes('.') ? name.slice(0, name.lastIndexOf('.')) : name
  const compact = stem.replace(/\s+/g, '')
  return GENERIC_MEDIA_NAME.test(stem) || OPAQUE_MEDIA_NAME.test(compact) || /^\d+$/.test(compact)
}

function urlNames(value = ''): string[] {
  try {
    const url = new URL(value)
    const candidates = ['filename', 'file', 'title', 'name', 'download']
      .map(key => cleanName(url.searchParams.get(key) || '', true))
      .filter(Boolean)
    const leaf = cleanName(url.pathname, true)
    return leaf ? [...candidates, leaf] : candidates
  } catch {
    return []
  }
}

export function suggestedResourceFilename(resource: Pick<MediaResource, 'kind' | 'url' | 'pageUrl' | 'title' | 'filename'>): string {
  if (resource.kind !== 'hls' && resource.kind !== 'dash') return cleanName(resource.filename || resource.title || '', true)
  const candidates = [
    cleanName(resource.filename || '', true),
    cleanName(resource.title || ''),
    ...urlNames(resource.pageUrl),
    ...urlNames(resource.url),
  ].filter(Boolean)
  return candidates.find(value => !isGenericMediaName(value)) || candidates[0] || 'download'
}

export function classifyResource(url: string, mimeType = ''): ResourceKind | null {
  if (url.startsWith('magnet:')) return 'magnet'
  if (!url.startsWith('http://') && !url.startsWith('https://')) return null
  if (SEGMENT_EXT.test(url)) return null
  const mime = mimeType.toLowerCase()
  if (/\.m3u8(?:$|[?#])/i.test(url) || mime.includes('mpegurl')) return 'hls'
  if (/\.mpd(?:$|[?#])/i.test(url) || mime.includes('dash+xml')) return 'dash'
  if (mime.startsWith('video/') || mime.startsWith('audio/')) return 'media'
  if (/\.torrent(?:$|[?#])/i.test(url) || mime.includes('bittorrent')) return 'file'
  return MEDIA_EXT.test(url) || mime.includes('octet-stream') ? 'file' : null
}

/** Remove LL-HLS blocking-reload cursors that expire after each playlist poll. */
export function canonicalMediaUrl(url: string, kind?: ResourceKind | null): string {
  const resourceKind = kind || classifyResource(url)
  if (resourceKind !== 'hls') return url
  return removeRawQueryParameters(url, LL_HLS_RELOAD_QUERY)
}

export interface PlaybackContext {
  sourceUrls: string[]
  startedAt: number
  mseResourceUrls?: string[]
}

function msePathAffinity(resourceUrl: string, mediaUrl: string): number {
  try {
    const resource = new URL(resourceUrl)
    const media = new URL(mediaUrl)
    if (resource.origin !== media.origin) return -1
    const resourceParts = resource.pathname.split('/').filter(Boolean).slice(0, -1)
    const mediaParts = media.pathname.split('/').filter(Boolean).slice(0, -1)
    let common = 0
    while (
      common < resourceParts.length
      && common < mediaParts.length
      && resourceParts[common] === mediaParts[common]
    ) common += 1
    return common
  } catch {
    return -1
  }
}

function mseCorrelatedResources(
  resources: MediaResource[],
  playback: PlaybackContext,
  limit: number,
): MediaResource[] {
  const evidence = playback.mseResourceUrls || []
  if (!evidence.length) return []
  const floor = playback.startedAt - 3 * 60_000
  const ranked = compactResources(resources, 40)
    .filter(item => (item.kind === 'hls' || item.kind === 'dash') && item.seenAt >= floor)
    .map(item => ({
      item,
      affinity: Math.max(...evidence.map(url => msePathAffinity(item.url, url))),
    }))
    // A same-origin match alone is not evidence: unrelated players and ads
    // frequently share one CDN host.
    .filter(entry => entry.affinity > 0)
  if (!ranked.length) return []
  const best = Math.max(...ranked.map(entry => entry.affinity))
  return ranked
    .filter(entry => entry.affinity === best)
    .sort((left, right) => resourceRank(right.item) - resourceRank(left.item)
      || right.item.seenAt - left.item.seenAt)
    .slice(0, limit)
    .map(entry => entry.item)
}

function isNonVideoManifest(resource: Pick<MediaResource, 'url'>): boolean {
  let pathnameAndQuery = resource.url
  try {
    const url = new URL(resource.url)
    pathnameAndQuery = `${decodeURIComponent(url.pathname)}?${decodeURIComponent(url.search)}`
    for (const [key, value] of url.searchParams) {
      if (['type', 'track', 'kind', 'media'].includes(key.toLowerCase()) && NON_VIDEO_MANIFEST_QUERY.has(value.toLowerCase())) return true
    }
  } catch {}
  return NON_VIDEO_MANIFEST_SIGNAL.test(pathnameAndQuery)
}

export function resourceFingerprint(resource: Pick<MediaResource, 'url' | 'kind'>): string {
  try {
    const url = new URL(canonicalMediaUrl(resource.url, resource.kind))
    url.hash = ''
    // A few CDN families use terse signature keys (s/e/_t) rather than the
    // conventional token/expires names.  Treat the trio as volatile only when
    // s and e occur together, so an ordinary semantic `e` query parameter on
    // another site is not silently discarded.
    const names = new Set([...url.searchParams.keys()].map(key => key.toLowerCase()))
    const hasShortLivedSignature = names.has('s') && names.has('e')
    const adaptiveOrMedia = ['hls', 'dash', 'media'].includes(resource.kind)
    for (const key of [...url.searchParams.keys()]) {
      if (
        VOLATILE_QUERY.test(key)
        || (adaptiveOrMedia && MEDIA_AUTH_QUERY.test(key))
        || (hasShortLivedSignature && ['s', 'e', '_t'].includes(key.toLowerCase()))
      ) {
        url.searchParams.delete(key)
      }
    }
    url.searchParams.sort()
    return `${resource.kind}:${url.href}`
  } catch {
    return `${resource.kind}:${resource.url.split('#', 1)[0]}`
  }
}

/**
 * Browser media elements and network observers can see the same stream with
 * different short-lived signatures (or a differently ordered query string).
 * Compare their stable fingerprint instead of demanding byte-for-byte URL
 * equality, but keep meaningful parameters such as quality in the key so two
 * separate renditions never become one-click equivalents.
 */
export function resourceMatchesPlaybackSource(resource: Pick<MediaResource, 'url' | 'kind'>, sourceUrl: string): boolean {
  if (!sourceUrl || sourceUrl.startsWith('blob:')) return false
  if (sourceUrl === resource.url) return true
  try {
    const source = new URL(sourceUrl)
    if (!['http:', 'https:'].includes(source.protocol)) return false
  } catch {
    return false
  }
  return resourceFingerprint(resource) === resourceFingerprint({ ...resource, url: sourceUrl })
}

export function isUsefulResource(resource: MediaResource): boolean {
  if (!resource.url || !resource.kind) return false
  if (resource.statusCode && (resource.statusCode < 200 || resource.statusCode >= 400)) return false
  if (resource.method && !['GET', 'POST'].includes(resource.method.toUpperCase())) return false
  if (resource.kind === 'magnet') return true
  if ((resource.kind === 'hls' || resource.kind === 'dash') && isNonVideoManifest(resource)) return false
  if (resource.kind === 'hls' || resource.kind === 'dash') return true
  let path = resource.url
  try { path = decodeURIComponent(new URL(resource.url).pathname) } catch {}
  if (SEGMENT_EXT.test(resource.url) || SEGMENT_MIME.test(resource.mimeType || '')) return false
  if (SEGMENT_PATH.test(path) && (!resource.size || resource.size < 8 * 1024 * 1024)) return false
  if (AD_SIGNAL.test(path) && (!resource.duration || resource.duration < 60) && (!resource.size || resource.size < 20 * 1024 * 1024)) return false
  return true
}

export function resourceRank(resource: MediaResource): number {
  let score = resource.kind === 'hls' ? 500 : resource.kind === 'dash' ? 480 : resource.kind === 'media' ? 300 : resource.kind === 'magnet' ? 250 : 100
  if (resource.variants?.length) score += 80
  if (resource.inspected) score += 15
  if (resource.isLive === true) score += 90
  else if (resource.isLive === false && resource.duration && resource.duration < 30) score -= 80
  if (resource.lowLatencyLive) score += 10
  if (resource.partOnlyLive) score -= 25
  if (resource.duration && resource.duration >= 60) score += 60
  else if (resource.duration && resource.duration >= 10) score += 20
  if (resource.height) score += Math.min(50, Math.round(resource.height / 40))
  if (resource.size && resource.size >= 20 * 1024 * 1024) score += 40
  else if (resource.size && resource.size >= 2 * 1024 * 1024) score += 15
  const likelyBytes = resource.size || resource.estimatedSize || 0
  if (likelyBytes >= 500 * 1024 * 1024) score += 30
  else if (likelyBytes >= 100 * 1024 * 1024) score += 20
  else if (likelyBytes >= 20 * 1024 * 1024) score += 10
  if (resource.title && !isGenericMediaName(resource.title)) score += 20
  return score
}

export function likelyResourceBytes(resource: Pick<MediaResource, 'size' | 'estimatedSize' | 'duration' | 'bandwidth'>): number {
  if (Number(resource.size) > 0) return Number(resource.size)
  if (Number(resource.estimatedSize) > 0) return Number(resource.estimatedSize)
  if (Number(resource.duration) > 0 && Number(resource.bandwidth) > 0) {
    return Math.round(Number(resource.duration) * Number(resource.bandwidth) / 8)
  }
  return 0
}

/**
 * Keep detection quiet until playback starts. Exact media-element sources are
 * strongest evidence. MSE/blob players expose no usable currentSrc, so retain
 * adaptive manifests fetched shortly before playback and throughout the same
 * playback session. Sites often preload a manifest well before the user
 * presses play or request a rendition after the initial buffering period.
 */
export function visiblePlaybackResources(
  resources: MediaResource[],
  playback: PlaybackContext | null,
  limit = 8,
  allowAdaptiveFallback = true,
): MediaResource[] {
  if (!playback) return []
  const sources = playback.sourceUrls.filter(Boolean)
  // Do not require the manifest to arrive in the first few seconds after play:
  // that dropped legitimate preloaded VOD streams and late quality switches.
  // We still require a user-started playback event and only accept adaptive
  // manifests; generic video responses remain too ambiguous for MSE players.
  const adaptiveSessionFloor = playback.startedAt - 3 * 60_000
  const candidates = compactResources(resources, 40)
    .filter(item => ['hls', 'dash', 'media'].includes(item.kind))
    .map(item => ({
      item,
      evidence: sources.some(source => resourceMatchesPlaybackSource(item, source)) ? 3
        : allowAdaptiveFallback && item.seenAt >= adaptiveSessionFloor && (item.kind === 'hls' || item.kind === 'dash') ? 2
          : 0,
      bytes: likelyResourceBytes(item),
    }))
    .filter(entry => entry.evidence > 0)
  const exact = candidates.filter(entry => entry.evidence === 3)
  let visible = exact.length ? exact : candidates
  if (!exact.length && visible.length > 1) {
    // Multiple raw manifests around one MSE player are ambiguous until at
    // least one has actually been parsed. A pre-roll often arrives first and
    // otherwise wins by recency/bitrate for a few seconds. Inspection is local
    // and bounded, so waiting here makes the per-video button accurate without
    // delaying the common single-manifest case.
    const inspected = visible.filter(entry => entry.item.inspected === true)
    // Give the first manifest probe a short grace period so an opening ad can
    // still be separated from the real stream. If the CDN rejects inspection,
    // expose the bounded candidates shortly afterwards instead of leaving the
    // overlay in an endless “正在识别” state.
    if (inspected.length) visible = inspected
    else if (Date.now() - playback.startedAt >= 1_500) visible = visible.slice(0, limit)
    else return []
    // A pre-roll is normally a short, end-listed VOD requested immediately
    // before the real live manifest. Once inspection has confirmed a live
    // playlist from this playback session, do not put those two unrelated
    // streams in the same player menu or let bitrate make the advert win.
    const recentLive = visible.filter(entry => entry.item.isLive === true
      && entry.item.inspected === true
      && entry.item.seenAt >= playback.startedAt - 10_000)
    if (recentLive.length) {
      // Live rendition polling refreshes the active stream continuously. Keep
      // streams in the newest poll cluster; an old preloaded/background live
      // manifest must not remain beside the currently advancing player.
      // Keep every verified live route in the newest observation cluster.
      // Some sites request both a PART-only low-latency route and a regular
      // completed-segment route for the same player. Silently discarding the
      // latter made the less stable route the only one-click choice.
      const livePool = recentLive
      const newestLiveObservation = Math.max(...livePool.map(entry => entry.item.seenAt))
      visible = livePool.filter(entry => newestLiveObservation - entry.item.seenAt <= 15_000)
    } else {
      const knownShortVod = (entry: typeof visible[number]) => entry.item.inspected === true
        && entry.item.isLive === false
        && Number(entry.item.duration) > 0
        && Number(entry.item.duration) < 45
      if (visible.some(entry => !knownShortVod(entry))) {
        visible = visible.filter(entry => !knownShortVod(entry))
      }
    }
  }
  visible.sort((left, right) => right.evidence - left.evidence
    || resourceRank(right.item) - resourceRank(left.item)
    || (right.item.height || 0) - (left.item.height || 0)
    || (right.item.bandwidth || 0) - (left.item.bandwidth || 0)
    || right.bytes - left.bytes
    || right.item.seenAt - left.item.seenAt)
  // IDM-style panel behavior: a real media-element source wins outright. For
  // MSE/blob playback, only adaptive manifests can represent the current
  // video; arbitrary video/* responses are often ads, thumbnails, or short
  // fMP4 pieces and must not become download candidates.
  return visible.slice(0, limit).map(entry => entry.item)
}

/**
 * Resolve candidates for one concrete HTMLMediaElement playback session.
 *
 * Direct URLs can be fingerprint-matched exactly. A blob/MSE URL intentionally
 * hides the manifest, so an adaptive fallback is only safe while it is the
 * sole active MSE session in that frame. This is deliberately conservative:
 * a missing button is preferable to putting another player's video beside the
 * wrong element.
 */
export function playerPlaybackResources(
  resources: MediaResource[],
  playback: PlaybackContext | null,
  activeMseSessions: number,
  limit = 8,
): MediaResource[] {
  const msePlayback = Boolean(playback?.sourceUrls.some(source => source.startsWith('blob:')))
  if (msePlayback && playback) {
    const correlated = mseCorrelatedResources(resources, playback, limit)
    if (correlated.length && (activeMseSessions <= 1 || correlated.length === 1)) {
      return correlated
    }
  }
  return visiblePlaybackResources(
    resources,
    playback,
    limit,
    // A concrete http(s) currentSrc must match that exact resource. Adaptive
    // time-window fallback exists only for blob/MSE, where the browser hides
    // the manifest URL from the media element.
    msePlayback && activeMseSessions <= 1,
  )
}

export function compactResources(resources: MediaResource[], limit = 40): MediaResource[] {
  const byKey = new Map<string, MediaResource>()
  for (const rawResource of resources) {
    const canonicalUrl = canonicalMediaUrl(rawResource.url, rawResource.kind)
    const resource = canonicalUrl === rawResource.url
      ? rawResource
      : { ...rawResource, url: canonicalUrl, id: resourceId(canonicalUrl) }
    if (!isUsefulResource(resource)) continue
    const key = resourceFingerprint(resource)
    const previous = byKey.get(key)
    if (!previous) {
      byKey.set(key, resource)
      continue
    }
    const newer = (resource.seenAt || 0) >= (previous.seenAt || 0) ? resource : previous
    const older = newer === resource ? previous : resource
    byKey.set(key, {
      ...older,
      ...newer,
      variants: newer.variants?.length ? newer.variants : older.variants,
      seenAt: Math.max(previous.seenAt || 0, resource.seenAt || 0),
    })
  }
  const result = [...byKey.values()]
  const childToParents = new Map<string, number[]>()
  result.forEach((item, parentIndex) => {
    for (const variant of item.variants || []) {
      const fingerprint = resourceFingerprint({ url: variant.url, kind: 'hls' })
      childToParents.set(fingerprint, [...(childToParents.get(fingerprint) || []), parentIndex])
    }
  })
  const refreshedParents = new Map<number, number>()
  for (const child of result) {
    const parents = childToParents.get(resourceFingerprint(child)) || []
    for (const parentIndex of parents) {
      refreshedParents.set(parentIndex, Math.max(
        refreshedParents.get(parentIndex) || 0,
        child.seenAt || 0,
      ))
    }
  }
  const refreshed = result.map((item, index) => {
    const seenAt = Math.max(item.seenAt || 0, refreshedParents.get(index) || 0)
    return seenAt === item.seenAt ? item : { ...item, seenAt }
  })
  const childVariants = new Set(refreshed.flatMap(item => item.variants || []).map(item => resourceFingerprint({ url: item.url, kind: 'hls' })))
  return refreshed
    .filter(item => Boolean(item.variants?.length) || !childVariants.has(resourceFingerprint(item)))
    .sort((left, right) => resourceRank(right) - resourceRank(left) || right.seenAt - left.seenAt)
    .slice(0, limit)
}

export function visibleMediaResources(resources: MediaResource[], limit = 8, fallbackToFiles = true): MediaResource[] {
  const compact = compactResources(resources, 40)
  const video = compact.filter(item => ['hls', 'dash', 'media'].includes(item.kind))
  return (video.length || !fallbackToFiles ? video : compact).slice(0, limit)
}

export function classifyDownload(
  url: string,
  mimeType = '',
  filename = '',
  contentDisposition = '',
): ResourceKind | null {
  const mime = mimeType.toLowerCase()
  const suppliedName = filename.split(/[\\/]/).pop() || ''
  const attachment = /^\s*attachment\b/i.test(contentDisposition)
  const suppliedIsImage = IMAGE_EXT.test(suppliedName)
  const suppliedIsDocument = DYNAMIC_DOCUMENT_EXT.test(suppliedName)
  const suppliedHasExtension = /\.[A-Za-z0-9]{1,10}(?:$|[?#])/.test(suppliedName)
  if (mime.startsWith('image/') || suppliedIsImage) return null
  // A browser DownloadItem alone is strong evidence for an actual download,
  // but pages also create DownloadItems for saved scripts and other passive
  // subresources. Keep those in the browser unless the server explicitly
  // marks the response as an attachment.
  if (!attachment && (PASSIVE_WEB_MIME.test(mime) || PASSIVE_WEB_EXT.test(url) || PASSIVE_WEB_EXT.test(suppliedName))) return null
  // Dynamic endpoints are common navigation and ad targets. Only take one over
  // when the server explicitly gives it a real, non-web download filename.
  if (DYNAMIC_DOCUMENT_EXT.test(url) && (!suppliedHasExtension || suppliedIsDocument)) return null
  if (mime.includes('octet-stream') && !MEDIA_EXT.test(url) && (!suppliedHasExtension || suppliedIsDocument)) return null
  const classified = classifyResource(url, mimeType)
    || classifyResource(`https://download.invalid/${encodeURIComponent(filename)}`, mimeType)
  if (classified) return classified
  const extension = filename.split(/[\\/]/).pop()?.match(/\.([A-Za-z0-9]{1,10})$/)?.[1]?.toLowerCase()
  if (extension && !['htm', 'html', 'xhtml'].includes(extension)) return 'file'
  if (mime && !mime.includes('text/html') && !mime.includes('application/xhtml')) return 'file'
  return null
}

export function resourceId(url: string): string {
  // Two independent 64-bit FNV-1a passes keep this synchronous for DOM/storage
  // keys while raising the identifier space from 32 to 128 bits. The complete
  // resourceFingerprint remains the authoritative Map key.
  const fnv64 = (value: string, offset: bigint): bigint => {
    let hash = offset
    for (let index = 0; index < value.length; index += 1) {
      hash ^= BigInt(value.charCodeAt(index))
      hash = BigInt.asUintN(64, hash * 1099511628211n)
    }
    return hash
  }
  const forward = fnv64(url, 14695981039346656037n)
  const reverse = fnv64([...url].reverse().join(''), 7809847782465536322n)
  return `${forward.toString(16).padStart(16, '0')}${reverse.toString(16).padStart(16, '0')}`
}

export function mergeResources(current: MediaResource[], incoming: MediaResource, limit = 100): MediaResource[] {
  const now = Date.now()
  return compactResources([incoming, ...current]
    .filter(item => now - item.seenAt < 30 * 60_000), limit)
}

export function shouldTakeover(input: {
  url: string
  sourcePageUrl?: string
  size?: number
  mimeType?: string
  filename?: string
  enabled: boolean
  minimumBytes: number
  excludedHosts: string[]
  explicitClick?: boolean
  strongEvidence?: boolean
  altBypass?: boolean
  ctrlForce?: boolean
}): boolean {
  if (input.altBypass) return false
  // OAuth and account-login navigations must never be pre-empted, even when a
  // stale intent or Ctrl click is present. This is a second boundary behind
  // the content-script filter because old session intents can survive briefly.
  if (isAuthenticationNavigation(input.url)) return false
  if (input.ctrlForce) return true
  if (!input.explicitClick && !input.strongEvidence) return false
  if (!input.enabled) return false
  try {
    const url = new URL(input.url)
    const hosts = [url.hostname, hostnameOf(input.sourcePageUrl || '')].filter(Boolean)
    const excluded = hosts.some(host => isExcludedHost(host, input.excludedHosts))
    if (!['http:', 'https:'].includes(url.protocol) || excluded) return false
  } catch {
    return false
  }
  // Like IDM/AB, an unknown Content-Length is eligible. A known small file is
  // left in the browser so favicons and tiny export responses are not captured.
  if (input.minimumBytes > 0 && Number(input.size) > 0 && Number(input.size) < input.minimumBytes) return false
  return true
}

export function pageIdentity(value = ''): string {
  if (!value) return ''
  try {
    const url = new URL(value)
    url.hash = ''
    return url.href
  } catch {
    return value.split('#', 1)[0]
  }
}

export function pageResourceKey(tabId: number, pageUrl = ''): string {
  const page = pageIdentity(pageUrl)
  if (tabId >= 0) return page ? `resources:tab:${tabId}:page:${resourceId(page)}` : `resources:tab:${tabId}`
  return `resources:page:${resourceId(page || 'global')}`
}

const UNSAFE_REPLAY_HEADERS = new Set([
  'accept-encoding', 'connection', 'content-length', 'cookie', 'host', 'keep-alive',
  'proxy-authenticate', 'proxy-authorization', 'range', 'te', 'trailer',
  'transfer-encoding', 'upgrade',
])

export function replayableRequestHeaders(values: Record<string, string> | undefined): Record<string, string> {
  const result: Record<string, string> = {}
  let total = 0
  for (const [rawName, rawValue] of Object.entries(values || {}).slice(0, 64)) {
    const name = rawName.trim().toLowerCase()
    const value = String(rawValue || '').trim()
    if (!name || !value || UNSAFE_REPLAY_HEADERS.has(name) || /[\r\n]/.test(name + value)) continue
    total += name.length + value.length
    if (total > 32 * 1024) break
    result[name] = value
  }
  return result
}

export function resourceRequestIdentity(
  resource: Pick<MediaResource, 'pageUrl' | 'requestHeaders'>,
  fallbackUserAgent = '',
): { referer: string; origin: string; userAgent: string } {
  const captured = Object.fromEntries(
    Object.entries(resource.requestHeaders || {}).map(([name, value]) => [name.toLowerCase(), String(value || '')]),
  )
  const pageOrigin = httpOrigin(resource.pageUrl || '')
  return {
    // Media access context belongs to the page in the browser address bar,
    // never to the manifest/CDN host.  Captured headers are only a fallback
    // for handoffs where no tab/page URL was available.
    referer: resource.pageUrl || captured.referer || '',
    origin: pageOrigin || captured.origin || '',
    userAgent: captured['user-agent'] || fallbackUserAgent,
  }
}

/** Canonical host form used by popup settings and the takeover boundary. */
export function normalizeHost(value = ''): string {
  const raw = String(value || '').trim().toLowerCase()
  if (!raw) return ''
  try {
    const parsed = raw.includes('://') ? new URL(raw) : new URL(`https://${raw}`)
    return parsed.hostname.replace(/^www\./, '')
  } catch {
    return raw.replace(/^\*\./, '').replace(/:\d+$/, '').replace(/^www\./, '').replace(/\.$/, '')
  }
}

function hostnameOf(value: string): string {
  return normalizeHost(value)
}

export function isExcludedHost(value: string, excludedHosts: string[]): boolean {
  const host = normalizeHost(value)
  if (!host) return false
  return excludedHosts.some(value => {
    const rule = normalizeHost(value)
    return Boolean(rule && (host === rule || host.endsWith(`.${rule}`)))
  })
}

/** Preserve the request identity the browser actually sent to one origin. */
export function capturedRequestIdentity(
  requestHeaders: Record<string, string> | undefined,
  fallbackUserAgent = '',
): { referer: string; origin: string; userAgent: string } {
  const captured = Object.fromEntries(
    Object.entries(requestHeaders || {}).map(([name, value]) => [name.toLowerCase(), String(value || '')]),
  )
  return {
    referer: captured.referer || '',
    origin: captured.origin || '',
    userAgent: captured['user-agent'] || fallbackUserAgent,
  }
}

export function matchesDownloadClick(
  intent: DownloadClickIntent,
  download: { url: string; finalUrl?: string; referrer?: string; chainUrls?: string[]; tabId?: number },
  now = Date.now(),
): boolean {
  if (isAuthenticationNavigation(intent.href)
    || [download.url, download.finalUrl, ...(download.chainUrls || [])].some(isAuthenticationNavigation)) return false
  const age = now - intent.at
  if (age < 0 || age > 7000) return false
  const sameTab = intent.tabId !== undefined && download.tabId !== undefined && intent.tabId === download.tabId
  const permittedNewTab = Boolean(intent.opensNewTab)
  if (intent.tabId !== undefined && download.tabId !== undefined && !sameTab && !permittedNewTab) return false
  const tabCompatible = sameTab || permittedNewTab || intent.tabId === undefined || download.tabId === undefined
  const samePage = Boolean(intent.pageUrl && download.referrer
    && stripHash(intent.pageUrl) === stripHash(download.referrer))
  // Positive evidence only: same page referrer, or same tab (Chrome often omits
  // DownloadItem.referrer but still exposes the initiating tab via webRequest).
  // Missing both is not enough to claim an unrelated generated download.
  const linked = samePage || sameTab
  if (intent.href) {
    const clicked = stripHash(intent.href)
    const page = intent.pageUrl ? stripHash(intent.pageUrl) : ''
    // Same-page anchors (# / page-local href) are not concrete download targets.
    // Keep generic same-page matching for those; only hard-fail concrete hrefs
    // that clearly point at a different tab.
    const pageLocalHref = Boolean(page && clicked === page)
    if (!pageLocalHref) {
      const candidates = [download.url, download.finalUrl, ...(download.chainUrls || [])]
        .filter((value): value is string => Boolean(value))
      const exact = candidates.some(value => stripHash(value) === clicked)
      // Exact gateway/CDN match: allow when tabs are compatible, even if Chrome
      // left referrer empty.
      if (exact && (sameTab || permittedNewTab || !intent.pageUrl || !download.referrer || samePage)) return true
      if (exact) return false
      // Many download buttons open a short-lived gateway URL, then the browser
      // reports only the final CDN file. Require same-tab or same-page evidence
      // so a random background download is never claimed.
      if (linked && tabCompatible && age <= 2500) return true
      return false
    }
  }
  if (intent.generic) {
    // Generic/button clicks have no concrete href. Prefer same-page evidence.
    // Chrome may omit referrer: allow only download-looking controls on the same
    // tab, never a bare "any click on this tab" claim.
    const limit = intent.ctrlForce ? 7000 : intent.controlHint ? 4500 : 2500
    const genericLinked = samePage || (Boolean(intent.controlHint) && sameTab)
    return age <= limit && genericLinked && tabCompatible
  }
  return age <= 3000 && samePage && tabCompatible
}

function stripHash(value: string): string {
  try {
    const url = new URL(value)
    url.hash = ''
    return url.href
  } catch {
    return value.split('#', 1)[0]
  }
}
