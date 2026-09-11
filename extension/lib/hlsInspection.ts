import { parseHlsManifest, resourceQuality, type HlsVariant } from './hlsManifest'
import { replayableRequestHeaders, type MediaResource } from './resources'
import { readBoundedResponseText } from './boundedResponse'

export type ManifestFetcher = (url: string, init: RequestInit) => Promise<Response>
const MAX_MANIFEST_BYTES = 2 * 1024 * 1024

export interface HlsInspectionResult {
  inspected: true
  manifestType: 'master' | 'media'
  isLive?: boolean
  lowLatencyLive?: boolean
  partOnlyLive?: boolean
  duration?: number
  variants: HlsVariant[]
  renditionUrls: string[]
  playbackUrls: string[]
  width?: number
  height?: number
  bandwidth?: number
  estimatedSize?: number
  quality?: string
}

type InspectionResource = Pick<MediaResource, 'url' | 'requestHeaders' | 'width' | 'height' | 'bandwidth' | 'estimatedSize'> & {
  /** Context headers are supplied by the background page only after a user/page association exists. */
  inspectionHeaders?: Record<string, string>
}

export function hlsInspectionHeaders(
  values: Record<string, string> | undefined,
  contextual: Record<string, string> | undefined = {},
): Record<string, string> {
  const headers = replayableRequestHeaders(values)
  for (const name of Object.keys(headers)) {
    if (['user-agent'].includes(name) || name.startsWith('sec-')) delete headers[name]
  }
  // Referer/Origin/Cookie are not replayed from arbitrary page payloads. They
  // can only enter this map through the background's exact resource context
  // (page URL + cookiesFor(resource URL)). Keep values bounded and reject
  // header injection before handing them to fetch().
  for (const [rawName, rawValue] of Object.entries(contextual || {})) {
    const name = String(rawName || '').trim().toLowerCase()
    const value = String(rawValue || '').trim()
    if (!['referer', 'origin', 'cookie'].includes(name) || !value || /[\r\n]/.test(value) || value.length > 32 * 1024) continue
    headers[name] = value
  }
  return headers
}

async function hlsManifestText(response: Response): Promise<string | null> {
  const text = await readBoundedResponseText(response, MAX_MANIFEST_BYTES)
  if (text === null) return null
  if (text.length > MAX_MANIFEST_BYTES || !/^\s*#EXTM3U(?:\s|$)/i.test(text)) return null
  return text
}

function estimatedBytes(duration: number | undefined, bandwidth: number | undefined): number | undefined {
  if (!Number.isFinite(duration) || (duration || 0) <= 0
    || !Number.isFinite(bandwidth) || (bandwidth || 0) <= 0) return undefined
  const bytes = (duration || 0) * (bandwidth || 0) / 8
  return Number.isFinite(bytes) && bytes > 0 ? Math.round(bytes) : undefined
}

/** Inspect a captured HLS resource without treating a live window as a VOD. */
export async function inspectHlsResource(
  resource: InspectionResource,
  fetcher: ManifestFetcher = (url, init) => fetch(url, init),
): Promise<HlsInspectionResult | null> {
  const headers = hlsInspectionHeaders(resource.requestHeaders, resource.inspectionHeaders)
  const fallbackHeaders = hlsInspectionHeaders(resource.requestHeaders)
  const hasContext = Object.keys(resource.inspectionHeaders || {}).length > 0
  const fetchManifest = async (url: string) => {
    const init = { credentials: 'include' as const, headers, signal: AbortSignal.timeout(5_000) }
    try {
      const response = await fetcher(url, init)
      if (response.ok || !hasContext) return response
      // A service worker/CORS policy may reject a manually supplied context
      // header even when the same URL is publicly inspectable. Retry once with
      // only replay-safe application headers instead of leaving the resource
      // permanently uninspected.
      return fetcher(url, { ...init, headers: fallbackHeaders })
    } catch (error) {
      if (!hasContext) throw error
      return fetcher(url, { ...init, headers: fallbackHeaders })
    }
  }
  const response = await fetchManifest(resource.url)
  if (!response.ok) return null
  const manifestText = await hlsManifestText(response)
  if (!manifestText) return null
  const info = parseHlsManifest(manifestText, response.url || resource.url)
  if (!info.duration && !info.variants.length && info.isLive === undefined) return null

  const videoCodec = /(?:avc|avc3|hev|hvc|vp8|vp9|av01|theora)/i
  const videoVariants = info.variants.filter(item => !item.codecs || videoCodec.test(item.codecs))
  const selectableVariants = videoVariants.length ? videoVariants : info.variants
  const variants = [...selectableVariants]
    .sort((left, right) => (right.height || 0) - (left.height || 0) || (right.bandwidth || 0) - (left.bandwidth || 0))
    .slice(0, 12)
  const best = variants[0]
  let live = info.isLive
  let lowLatencyLive = info.lowLatencyLive
  let partOnlyLive = info.partOnlyLive
  let duration = live ? undefined : info.duration
  let playbackUrls = info.playbackUrls
  if (!duration && best) {
    try {
      const mediaResponse = await fetchManifest(best.url)
      if (mediaResponse.ok) {
        const mediaText = await hlsManifestText(mediaResponse)
        if (mediaText) {
          const mediaInfo = parseHlsManifest(mediaText, mediaResponse.url || best.url)
          live = mediaInfo.isLive
          lowLatencyLive = mediaInfo.lowLatencyLive
          partOnlyLive = mediaInfo.partOnlyLive
          playbackUrls = mediaInfo.playbackUrls
          if (live === false) duration = mediaInfo.duration
        }
      }
    } catch {}
  }
  const bandwidth = best?.bandwidth || resource.bandwidth
  const fallbackEstimatedSize = typeof resource.estimatedSize === 'number'
    && Number.isFinite(resource.estimatedSize) && resource.estimatedSize > 0
    ? resource.estimatedSize
    : undefined
  return {
    inspected: true,
    manifestType: info.variants.length ? 'master' : 'media',
    isLive: live,
    lowLatencyLive,
    partOnlyLive,
    duration,
    variants,
    renditionUrls: info.renditionUrls,
    playbackUrls,
    width: best?.width || resource.width,
    height: best?.height || resource.height,
    bandwidth,
    estimatedSize: live
      ? undefined
      : duration && bandwidth
        ? estimatedBytes(duration, bandwidth)
        : fallbackEstimatedSize,
    quality: best?.quality ? `最高 ${best.quality}` : resourceQuality(resource.url, resource.height),
  }
}
