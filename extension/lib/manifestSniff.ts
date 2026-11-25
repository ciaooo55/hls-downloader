/** Lightweight browser-side manifest sniffing for extensionless media URLs. */

export type ManifestKind = 'hls' | 'dash'

/** A media payload, partial payload or browser cache hit; never a redirect/error body. */
export function isMediaResponseStatus(status: number): boolean {
  return status === 200 || status === 206 || status === 304
}

const URL_HINT = /(?:\.m3u8?(?:$|[?#])|\.mpd(?:$|[?#])|(?:^|[\/_?.=-])(?:hls|dash|manifest|playlist|master|chunklist)(?:$|[\/_?.=-]))/i
const MANIFEST_EXT_URL = /\.(?:m3u8?|mpd)(?:$|[?#])/i
const MEDIA_URL = /\.(?:m3u8?|mpd|mp4|m4v|m4a|webm|mkv|mov|avi|flv|mp3|aac|flac|ogg|opus|wav)(?:$|[?#])/i
const MEDIA_MIME = /^(?:audio|video)\//i
const DIRECT_STREAM_PATH = /\/videoplayback(?:\/|$)/i
const GOOGLE_VIDEO_HOST = /(?:^|\.)googlevideo\.(?:com|test)$/i
const YOUTUBE_MUXED_ITAGS = new Set([
  '5', '6', '17', '18', '22', '34', '35', '36', '37', '38',
  '43', '44', '45', '46', '59', '78', '82', '83', '84', '85',
  '100', '101', '102',
])
const FMP4_INIT_SEGMENT_PATH = /(?:^|[\/_-])(?:init(?:[-_][a-z0-9]+)?|map)\.(?:mp4|m4s|cmfv|cmfa|webm)$/i
const API_ROUTE_PATH = /(?:^|\/)(?:api|ajax|graphql|x\/player|player\/playurl|playurl)(?:\/|$)/i
const PASSIVE_RESPONSE_MIME = /^(?:application\/(?:ecmascript|javascript|json|ld\+json|manifest\+json|wasm)|(?:font|image)\/[^;]+|text\/(?:css|html|javascript))$/i

function truthyQueryFlag(value: string | null): boolean {
  if (!value) return false
  const normalized = value.trim().toLowerCase()
  return normalized === '1' || normalized === 'true' || normalized === 'yes'
}

/**
 * True for raw MSE byte-slice / SABR / UMP / unmuxed adaptive chunk URLs and
 * fMP4 init segments that cannot be played or downloaded standalone.
 *
 * Genuine `.m3u8` / `.mpd` manifests (including YouTube Live manifests) and
 * progressive muxed `/videoplayback?itag=18` URLs return `false`.
 */
export function isMseMediaSliceUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:'].includes(parsed.protocol)) return false
    if (MANIFEST_EXT_URL.test(parsed.pathname)) return false
    if (FMP4_INIT_SEGMENT_PATH.test(parsed.pathname)) return true

    const params = new Map<string, string>()
    parsed.searchParams.forEach((val, key) => {
      const lower = key.toLowerCase()
      if (!params.has(lower)) params.set(lower, val)
    })

    if (
      (params.get('range') || '').trim() !== ''
      || (params.get('bytestart') || '').trim() !== ''
      || (params.get('byteend') || '').trim() !== ''
      || truthyQueryFlag(params.get('sabr') ?? null)
      || truthyQueryFlag(params.get('ump') ?? null)
    ) {
      return true
    }

    if (DIRECT_STREAM_PATH.test(parsed.pathname)) {
      if (
        truthyQueryFlag(params.get('gir') ?? null)
        || (params.get('aitags') || '').trim() !== ''
        || (params.get('sq') || '').trim() !== ''
        || ((params.get('rn') || '').trim() !== '' && (params.get('rbuf') || '').trim() !== '')
      ) {
        return true
      }
      const mime = (params.get('mime') || '').trim().toLowerCase()
      const itag = (params.get('itag') || '').trim()
      const isGoogleVideo = GOOGLE_VIDEO_HOST.test(parsed.hostname)
      if (isGoogleVideo || itag || mime) {
        if (mime.startsWith('audio/')) return true
        if (itag && /^\d+$/.test(itag) && !YOUTUBE_MUXED_ITAGS.has(itag)) return true
        if (isGoogleVideo && mime.startsWith('video/') && !itag) return true
      }
    }
    return false
  } catch {
    return false
  }
}

/**
 * Extensionless media CDNs used by current video sites.
 * YouTube/googlevideo: `/videoplayback?mime=video%2Fmp4&itag=…`
 * Do not match JSON playurl APIs, raw MSE/SABR slices, or `.m4s` fragments.
 */
export function isCommonMediaStreamUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:'].includes(parsed.protocol)) return false
    if (isMseMediaSliceUrl(url)) return false
    if (DIRECT_STREAM_PATH.test(parsed.pathname)) return true
    // A JSON playurl API often echoes `mime=video/mp4` as a selector, but its
    // response is metadata rather than downloadable media.  Reject API-shaped
    // routes even before response headers arrive; concrete CDN endpoints such
    // as `/get?mime=audio/webm` remain eligible.
    if (API_ROUTE_PATH.test(parsed.pathname)) return false
    const mime = (parsed.searchParams.get('mime') || parsed.searchParams.get('content_type') || '').toLowerCase()
    return mime.startsWith('video/') || mime.startsWith('audio/')
  } catch {
    return false
  }
}

export function shouldInspectManifestResponse(url: string, mimeType = ''): boolean {
  const mime = String(mimeType || '').split(';', 1)[0].trim().toLowerCase()
  if (mime.includes('mpegurl') || mime.includes('dash+xml')) return true
  // Avoid cloning ordinary JSON/HTML/image responses merely because an API
  // route contains words such as playlist or manifest.  Unknown, text/plain,
  // XML and octet-stream responses remain bounded-inspection candidates.
  if (PASSIVE_RESPONSE_MIME.test(mime)) return false
  // Do not clone arbitrary MP4/octet-stream responses.  URL hints cover the
  // common extensionless CDN endpoints while keeping the extra read bounded.
  return URL_HINT.test(String(url || ''))
}

/**
 * Keep the MAIN-world bridge quiet on ordinary application traffic.
 *
 * fetch/XHR hooks run before the isolated content script can decide whether a
 * response is useful. Dispatching one DOM event for every JSON, script, image
 * and analytics request made idle pages surprisingly expensive. Direct media,
 * known manifests and manifest-like endpoints are sufficient here; MSE byte
 * ownership is reported through its separate, exact bridge.
 */
export function shouldReportMediaResponse(url: string, mimeType = ''): boolean {
  const mime = String(mimeType || '').split(';', 1)[0].trim().toLowerCase()
  const rawUrl = String(url || '')
  if (
    mime
    && !mime.includes('mpegurl')
    && !mime.includes('dash+xml')
    && !MEDIA_MIME.test(mime)
    && /^(?:application\/(?:ecmascript|javascript|json|ld\+json|manifest\+json|wasm|xml)|font\/|text\/(?:css|html|javascript|xml)|image\/)/i.test(mime)
  ) {
    return false
  }
  if (mime.includes('mpegurl') || mime.includes('dash+xml') || MANIFEST_EXT_URL.test(rawUrl)) return true
  if (isMseMediaSliceUrl(rawUrl)) return shouldInspectManifestResponse(rawUrl, mime)
  return MEDIA_MIME.test(mime)
    || MEDIA_URL.test(rawUrl)
    || isCommonMediaStreamUrl(rawUrl)
    || shouldInspectManifestResponse(rawUrl, mime)
}

export function detectManifestKind(prefix: string): ManifestKind | null {
  let value = String(prefix || '').replace(/^\s*\uFEFF/, '').trimStart()
  if (/^#EXTM3U[\t ]*(?:\r?\n|$)/.test(value)) return 'hls'
  value = value.replace(/^<\?xml(?:\s+[^?]*?)?\?>\s*/i, '')
  // Only skip the XML prolog; a marker inside an HTML/error body is not media.
  while (value.startsWith('<!--')) {
    const end = value.indexOf('-->', 4)
    if (end < 0) return null
    value = value.slice(end + 3).trimStart()
  }
  if (/^<(?:[A-Za-z_][\w.-]*:)?MPD(?:\s|>)/i.test(value)) return 'dash'
  return null
}

export function manifestMimeType(kind: ManifestKind): string {
  return kind === 'hls' ? 'application/vnd.apple.mpegurl' : 'application/dash+xml'
}
