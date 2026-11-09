import { parseHlsManifest, resourceQuality, type HlsVariant } from './hlsManifest'
import { type MediaResource, type MediaAudioTrack } from './resources'
import { createManifestFetcher, estimatedManifestBytes, type ManifestFetcher, type ManifestRequestContext } from './manifestInspection'
import { readBoundedResponseText } from './boundedResponse'

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
  audioTracks: MediaAudioTrack[]
  playbackUrls: string[]
  playbackUrlHashes: string[]
  width?: number
  height?: number
  bandwidth?: number
  estimatedSize?: number
  quality?: string
}

type InspectionResource = Pick<MediaResource, 'width' | 'height' | 'bandwidth' | 'estimatedSize'> & ManifestRequestContext

async function hlsManifestText(response: Response): Promise<string | null> {
  const text = await readBoundedResponseText(response, MAX_MANIFEST_BYTES)
  if (text === null) return null
  if (text.length > MAX_MANIFEST_BYTES || !/^\s*#EXTM3U(?:\s|$)/i.test(text)) return null
  return text
}

/** Inspect a captured HLS resource without treating a live window as a VOD. */
export async function inspectHlsResource(
  resource: InspectionResource,
  fetcher?: ManifestFetcher,
): Promise<HlsInspectionResult | null> {
  const fetchManifest = createManifestFetcher(resource, fetcher)
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
  let playbackUrlHashes = info.playbackUrlHashes
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
          playbackUrlHashes = mediaInfo.playbackUrlHashes
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
    audioTracks: info.audioTracks,
    playbackUrls,
    playbackUrlHashes,
    width: best?.width || resource.width,
    height: best?.height || resource.height,
    bandwidth,
    estimatedSize: live
      ? undefined
      : duration && bandwidth
        ? estimatedManifestBytes(duration, bandwidth)
        : fallbackEstimatedSize,
    quality: best?.quality ? `最高 ${best.quality}` : resourceQuality(resource.url, resource.height),
  }
}
