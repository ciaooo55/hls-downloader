export type ResourceKind = 'hls' | 'dash' | 'media' | 'file' | 'magnet'

export interface MediaResource {
  id: string
  url: string
  kind: ResourceKind
  mimeType?: string
  size?: number
  pageUrl?: string
  title?: string
  filename?: string
  statusCode?: number
  method?: string
  seenAt: number
}

export interface DownloadClickIntent {
  href: string
  pageUrl: string
  altBypass: boolean
  ctrlForce: boolean
  at: number
}

const MEDIA_EXT = /\.(m3u8|mpd|mp4|webm|mkv|mov|avi|m4a|mp3|flac|wav|zip|7z|rar|exe|msi|pdf)(?:$|[?#])/i
const SEGMENT_EXT = /\.(?:ts|m4s|cmfv|cmfa|aac)(?:$|[?#])/i
const DOWNLOAD_PATH_HINT = /(?:^|\/)(?:download|downloads|dl|export)(?:\.[a-z0-9]+)?(?:\/|$)/i
const DOWNLOAD_QUERY_HINT = /^(?:download|dl|attachment|export)$/i
const FILE_QUERY_HINT = /^(?:file|filename|path)$/i

export function classifyResource(url: string, mimeType = ''): ResourceKind | null {
  if (url.startsWith('magnet:')) return 'magnet'
  if (!url.startsWith('http://') && !url.startsWith('https://')) return null
  if (SEGMENT_EXT.test(url)) return null
  const mime = mimeType.toLowerCase()
  if (/\.m3u8(?:$|[?#])/i.test(url) || mime.includes('mpegurl')) return 'hls'
  if (/\.mpd(?:$|[?#])/i.test(url) || mime.includes('dash+xml')) return 'dash'
  if (mime.startsWith('video/') || mime.startsWith('audio/')) return 'media'
  return MEDIA_EXT.test(url) || mime.includes('octet-stream') ? 'file' : null
}

export function classifyDownload(url: string, mimeType = '', filename = ''): ResourceKind | null {
  const classified = classifyResource(url, mimeType)
    || classifyResource(`https://download.invalid/${encodeURIComponent(filename)}`, mimeType)
  if (classified) return classified
  const extension = filename.split(/[\\/]/).pop()?.match(/\.([A-Za-z0-9]{1,10})$/)?.[1]?.toLowerCase()
  if (extension && !['htm', 'html', 'xhtml'].includes(extension)) return 'file'
  const mime = mimeType.toLowerCase()
  if (mime && !mime.includes('text/html') && !mime.includes('application/xhtml')) return 'file'
  return null
}

export function resourceId(url: string): string {
  let hash = 2166136261
  for (let index = 0; index < url.length; index += 1) {
    hash ^= url.charCodeAt(index)
    hash = Math.imul(hash, 16777619)
  }
  return (hash >>> 0).toString(36)
}

export function mergeResources(current: MediaResource[], incoming: MediaResource, limit = 100): MediaResource[] {
  const previous = current.find(item => item.url === incoming.url)
  const merged = previous ? { ...previous, ...incoming, seenAt: Date.now() } : incoming
  return [merged, ...current.filter(item => item.url !== incoming.url)]
    .filter(item => Date.now() - item.seenAt < 30 * 60_000)
    .slice(0, limit)
}

export function shouldTakeover(input: {
  url: string
  size?: number
  mimeType?: string
  filename?: string
  enabled: boolean
  minimumBytes: number
  excludedHosts: string[]
  explicitClick?: boolean
  altBypass?: boolean
  ctrlForce?: boolean
}): boolean {
  if (input.altBypass) return false
  if (input.ctrlForce) return true
  if (!input.explicitClick) return false
  if (!input.enabled) return false
  try {
    const url = new URL(input.url)
    if (!['http:', 'https:'].includes(url.protocol) || input.excludedHosts.includes(url.host)) return false
  } catch {
    return false
  }
  return true
}

export function isDirectDownloadLink(url: string, hasDownloadAttribute = false): boolean {
  if (hasDownloadAttribute || classifyDownload(url) !== null) return true
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:'].includes(parsed.protocol)) return false
    if (DOWNLOAD_PATH_HINT.test(parsed.pathname)) return true
    for (const [key, value] of parsed.searchParams) {
      if (DOWNLOAD_QUERY_HINT.test(key) && !/^(?:0|false|no)$/i.test(value)) return true
      if (FILE_QUERY_HINT.test(key) && classifyDownload(`https://download.invalid/${value}`) !== null) return true
      if (/^(?:action|do|mode)$/i.test(key) && /^(?:download|export)$/i.test(value)) return true
    }
  } catch {
    return false
  }
  return false
}

export function matchesDownloadClick(
  intent: DownloadClickIntent,
  download: { url: string; finalUrl?: string },
  now = Date.now(),
): boolean {
  if (now - intent.at < 0 || now - intent.at > 7000) return false
  if (!intent.href) return false
  const clicked = stripHash(intent.href)
  return [download.url, download.finalUrl]
    .filter((value): value is string => Boolean(value))
    .some(value => stripHash(value) === clicked)
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
