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
  tabId?: number
  statusCode?: number
  method?: string
  requestHeaders?: Record<string, string>
  width?: number
  height?: number
  bandwidth?: number
  quality?: string
  duration?: number
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

const MEDIA_EXT = /\.(m3u8|mpd|mp4|webm|mkv|mov|avi|m4a|mp3|flac|wav|zip|7z|rar|exe|msi|pdf)(?:$|[?#])/i
const SEGMENT_EXT = /\.(?:ts|m4s|cmfv|cmfa|aac)(?:$|[?#])/i
const MANIFEST_EXT = /\.(?:m3u8?|mpd)$/i
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
    const host = url.host.toLowerCase()
    const excluded = input.excludedHosts.some(value => {
      const rule = String(value || '').trim().toLowerCase().replace(/^\*\./, '')
      return Boolean(rule && (host === rule || host.endsWith(`.${rule}`)))
    })
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
  return {
    // A page URL is a valid fallback for Referer when the resource came from
    // DOM/performance capture. Origin is deliberately not synthesized: normal
    // GET navigations/downloads often omit it and some CDNs reject an invented one.
    referer: captured.referer || resource.pageUrl || '',
    origin: captured.origin || '',
    userAgent: captured['user-agent'] || fallbackUserAgent,
  }
}

export function matchesDownloadClick(
  intent: DownloadClickIntent,
  download: { url: string; finalUrl?: string; referrer?: string; chainUrls?: string[]; tabId?: number },
  now = Date.now(),
): boolean {
  const age = now - intent.at
  if (age < 0 || age > 7000) return false
  const sameTab = intent.tabId !== undefined && download.tabId !== undefined && intent.tabId === download.tabId
  const permittedNewTab = Boolean(intent.opensNewTab)
  if (intent.tabId !== undefined && download.tabId !== undefined && !sameTab && !permittedNewTab) return false
  const samePage = Boolean(intent.pageUrl && download.referrer
    && stripHash(intent.pageUrl) === stripHash(download.referrer))
  if (intent.href) {
    const clicked = stripHash(intent.href)
    const exact = [download.url, download.finalUrl, ...(download.chainUrls || [])]
      .filter((value): value is string => Boolean(value))
      .some(value => stripHash(value) === clicked)
    if (exact && (sameTab || permittedNewTab || !intent.pageUrl || !download.referrer || samePage)) return true
    if (exact) return false
  }
  if (intent.generic) {
    const limit = intent.ctrlForce ? 7000 : intent.controlHint ? 4000 : 1000
    return age <= limit
      && samePage
      && (sameTab || permittedNewTab || intent.tabId === undefined || download.tabId === undefined)
  }
  return age <= 3000 && samePage && (sameTab || permittedNewTab || intent.tabId === undefined || download.tabId === undefined)
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
