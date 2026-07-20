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
  requestHeaders?: Record<string, string>
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
}

const MEDIA_EXT = /\.(m3u8|mpd|mp4|webm|mkv|mov|avi|m4a|mp3|flac|wav|zip|7z|rar|exe|msi|pdf)(?:$|[?#])/i
const SEGMENT_EXT = /\.(?:ts|m4s|cmfv|cmfa|aac)(?:$|[?#])/i

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

export function matchesDownloadClick(
  intent: DownloadClickIntent,
  download: { url: string; finalUrl?: string; referrer?: string; chainUrls?: string[]; tabId?: number },
  now = Date.now(),
): boolean {
  const age = now - intent.at
  if (age < 0 || age > 7000) return false
  const sameTab = intent.tabId !== undefined && download.tabId !== undefined && intent.tabId === download.tabId
  if (intent.tabId !== undefined && download.tabId !== undefined && !sameTab) return false
  const samePage = Boolean(intent.pageUrl && download.referrer
    && stripHash(intent.pageUrl) === stripHash(download.referrer))
  if (intent.href) {
    if (intent.pageUrl && download.referrer && !samePage) return false
    const clicked = stripHash(intent.href)
    const exact = [download.url, download.finalUrl, ...(download.chainUrls || [])]
      .filter((value): value is string => Boolean(value))
      .some(value => stripHash(value) === clicked)
    if (exact) return true
  }
  if (intent.generic) return age <= 1500 && (samePage || sameTab)
  return age <= 3000 && samePage && (sameTab || intent.tabId === undefined || download.tabId === undefined)
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
