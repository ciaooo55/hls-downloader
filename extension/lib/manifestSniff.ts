/** Lightweight browser-side manifest sniffing for extensionless media URLs. */

export type ManifestKind = 'hls' | 'dash'

const URL_HINT = /(?:\.m3u8?(?:$|[?#])|\.mpd(?:$|[?#])|(?:^|[\/_?.=-])(?:hls|dash|manifest|playlist|master|chunklist)(?:$|[\/_?.=-]))/i

export function shouldInspectManifestResponse(url: string, mimeType = ''): boolean {
  const mime = String(mimeType || '').toLowerCase()
  if (mime.includes('mpegurl') || mime.includes('dash+xml')) return true
  // Do not clone arbitrary MP4/octet-stream responses.  URL hints cover the
  // common extensionless CDN endpoints while keeping the extra read bounded.
  return URL_HINT.test(String(url || ''))
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
