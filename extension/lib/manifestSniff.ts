/** Lightweight browser-side manifest sniffing for extensionless media URLs. */

export type ManifestKind = 'hls' | 'dash'

const URL_HINT = /(?:\.m3u8?(?:$|[?#])|\.mpd(?:$|[?#])|(?:^|[\/_?.=-])(?:hls|dash|manifest|playlist|master|chunklist)(?:$|[\/_?.=-]))/i
const MEDIA_URL = /\.(?:m3u8?|mpd|mp4|m4v|m4a|webm|mkv|mov|avi|flv|mp3|aac|flac|ogg|opus|wav)(?:$|[?#])/i
const MEDIA_MIME = /^(?:audio|video)\//i
const DIRECT_STREAM_PATH = /\/videoplayback(?:\/|$)/i

/**
 * Extensionless media CDNs used by current video sites.
 * YouTube/googlevideo: `/videoplayback?mime=video%2Fmp4&itag=…`
 * Do not match JSON playurl APIs or `.m4s` fragments.
 */
export function isCommonMediaStreamUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:'].includes(parsed.protocol)) return false
    if (DIRECT_STREAM_PATH.test(parsed.pathname)) return true
    const mime = (parsed.searchParams.get('mime') || parsed.searchParams.get('content_type') || '').toLowerCase()
    return mime.startsWith('video/') || mime.startsWith('audio/')
  } catch {
    return false
  }
}

export function shouldInspectManifestResponse(url: string, mimeType = ''): boolean {
  const mime = String(mimeType || '').toLowerCase()
  if (mime.includes('mpegurl') || mime.includes('dash+xml')) return true
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
  if (
    mime
    && !mime.includes('mpegurl')
    && !mime.includes('dash+xml')
    && !MEDIA_MIME.test(mime)
    && /^(?:application\/(?:ecmascript|javascript|json|ld\+json|manifest\+json|wasm|xml)|font\/|text\/(?:css|html|javascript|xml)|image\/)/i.test(mime)
  ) {
    return false
  }
  return MEDIA_MIME.test(mime)
    || mime.includes('mpegurl')
    || mime.includes('dash+xml')
    || MEDIA_URL.test(String(url || ''))
    || isCommonMediaStreamUrl(String(url || ''))
    || shouldInspectManifestResponse(url, mime)
}

export function detectManifestKind(prefix: string): ManifestKind | null {
  const value = String(prefix || '').replace(/^\s*\uFEFF/, '').trimStart()
  if (value.startsWith('#EXTM3U')) return 'hls'
  if (/^<(?:[A-Za-z_][\w.-]*:)?MPD(?:\s|>)/i.test(value)) return 'dash'
  return null
}

export function manifestMimeType(kind: ManifestKind): string {
  return kind === 'hls' ? 'application/vnd.apple.mpegurl' : 'application/dash+xml'
}
