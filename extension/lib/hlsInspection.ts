import { parseHlsManifest, resourceQuality, type HlsVariant } from './hlsManifest'
import { replayableRequestHeaders, type MediaResource } from './resources'

export type ManifestFetcher = (url: string, init: RequestInit) => Promise<Response>

export interface HlsInspectionResult {
  inspected: true
  manifestType: 'master' | 'media'
  isLive?: boolean
  lowLatencyLive?: boolean
  partOnlyLive?: boolean
  duration?: number
  variants: HlsVariant[]
  width?: number
  height?: number
  bandwidth?: number
  estimatedSize?: number
  quality?: string
}

export function hlsInspectionHeaders(values: Record<string, string> | undefined): Record<string, string> {
  const headers = replayableRequestHeaders(values)
  for (const name of Object.keys(headers)) {
    if (['referer', 'origin', 'user-agent'].includes(name) || name.startsWith('sec-')) delete headers[name]
  }
  return headers
}

/** Inspect a captured HLS resource without treating a live window as a VOD. */
export async function inspectHlsResource(
  resource: Pick<MediaResource, 'url' | 'requestHeaders' | 'width' | 'height' | 'bandwidth' | 'estimatedSize'>,
  fetcher: ManifestFetcher = (url, init) => fetch(url, init),
): Promise<HlsInspectionResult | null> {
  const headers = hlsInspectionHeaders(resource.requestHeaders)
  const fetchManifest = (url: string) => fetcher(url, {
    credentials: 'include',
    headers,
    signal: AbortSignal.timeout(5_000),
  })
  const response = await fetchManifest(resource.url)
  if (!response.ok) return null
  const info = parseHlsManifest(await response.text(), response.url || resource.url)
  if (!info.duration && !info.variants.length && info.isLive === undefined) return null

  const variants = [...info.variants]
    .sort((left, right) => (right.height || 0) - (left.height || 0) || (right.bandwidth || 0) - (left.bandwidth || 0))
    .slice(0, 12)
  const best = variants[0]
  let live = info.isLive
  let lowLatencyLive = info.lowLatencyLive
  let partOnlyLive = info.partOnlyLive
  let duration = live ? undefined : info.duration
  if (!duration && best) {
    const mediaResponse = await fetchManifest(best.url)
    if (mediaResponse.ok) {
      const mediaInfo = parseHlsManifest(await mediaResponse.text(), mediaResponse.url || best.url)
      live = mediaInfo.isLive
      lowLatencyLive = mediaInfo.lowLatencyLive
      partOnlyLive = mediaInfo.partOnlyLive
      if (live === false) duration = mediaInfo.duration
    }
  }
  const bandwidth = best?.bandwidth || resource.bandwidth
  return {
    inspected: true,
    manifestType: info.variants.length ? 'master' : 'media',
    isLive: live,
    lowLatencyLive,
    partOnlyLive,
    duration,
    variants,
    width: best?.width || resource.width,
    height: best?.height || resource.height,
    bandwidth,
    estimatedSize: live
      ? undefined
      : duration && bandwidth
        ? Math.round(duration * bandwidth / 8)
        : resource.estimatedSize,
    quality: best?.quality ? `最高 ${best.quality}` : resourceQuality(resource.url, resource.height),
  }
}
