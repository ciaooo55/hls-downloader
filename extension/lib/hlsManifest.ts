import { inheritManifestAccessQuery } from './urlQuery'

export interface HlsVariant {
  url: string
  width?: number
  height?: number
  bandwidth?: number
  codecs?: string
  quality?: string
}

export interface HlsManifestInfo {
  variants: HlsVariant[]
  /** Alternate audio/video/subtitle playlists owned by this master. */
  renditionUrls: string[]
  /** Bounded media/init URLs used to associate an MSE SourceBuffer with this playlist. */
  playbackUrls: string[]
  duration?: number
  /** Present only for media playlists; a master cannot determine liveness. */
  isLive?: boolean
  /** LL-HLS media playlists advertise partial segments/control directives. */
  lowLatencyLive?: boolean
  /** The current live window contains PART tags but no completed EXTINF segment. */
  partOnlyLive?: boolean
}

function attribute(line: string, name: string): string {
  const match = line.match(new RegExp(`(?:^|,)${name}=("[^"]*"|[^,]*)`, 'i'))
  return (match?.[1] || '').replace(/^"|"$/g, '')
}

function positiveFiniteNumber(value: string): number | undefined {
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined
}

function followingUri(
  lines: string[],
  index: number,
  boundary: (line: string) => boolean,
): string {
  for (let cursor = index + 1; cursor < lines.length; cursor += 1) {
    const candidate = lines[cursor]
    if (!candidate.startsWith('#')) return candidate
    if (boundary(candidate)) return ''
  }
  return ''
}

export function parseHlsManifest(text: string, baseUrl: string): HlsManifestInfo {
  const lines = text.split(/\r?\n/).map(line => line.trim()).filter(Boolean)
  const variants: HlsVariant[] = []
  const renditionUrls: string[] = []
  const playbackUrls: string[] = []
  const rememberPlaybackUrl = (value: string) => {
    if (!value) return
    try {
      const resolved = inheritManifestAccessQuery(baseUrl, new URL(value, baseUrl).href)
      if (!playbackUrls.includes(resolved)) playbackUrls.push(resolved)
    } catch {}
  }
  let duration = 0
  let completeDuration = true
  let completeSegments = 0
  let partialSegments = 0
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]
    if (line.startsWith('#EXT-X-MEDIA:')) {
      const uri = attribute(line.slice('#EXT-X-MEDIA:'.length), 'URI')
      if (uri) {
        try {
          const resolved = inheritManifestAccessQuery(baseUrl, new URL(uri, baseUrl).href)
          if (!renditionUrls.includes(resolved)) renditionUrls.push(resolved)
        } catch {}
      }
    }
    if (line.startsWith('#EXTINF:')) {
      const durationText = line.slice(8).split(',', 1)[0].trim()
      const segmentDuration = durationText ? Number(durationText) : Number.NaN
      const uri = followingUri(lines, index, candidate =>
        candidate.startsWith('#EXTINF:') || candidate === '#EXT-X-ENDLIST')
      if (Number.isFinite(segmentDuration) && segmentDuration >= 0 && uri) {
        const nextDuration = duration + segmentDuration
        if (Number.isFinite(nextDuration)) duration = nextDuration
        else completeDuration = false
      } else completeDuration = false
      completeSegments += 1
      if (uri) rememberPlaybackUrl(uri)
    }
    if (line.startsWith('#EXT-X-PART:')) {
      partialSegments += 1
      rememberPlaybackUrl(attribute(line.slice('#EXT-X-PART:'.length), 'URI'))
    }
    if (line.startsWith('#EXT-X-MAP:')) {
      rememberPlaybackUrl(attribute(line.slice('#EXT-X-MAP:'.length), 'URI'))
    }
    if (line.startsWith('#EXT-X-PRELOAD-HINT:') && attribute(line.slice('#EXT-X-PRELOAD-HINT:'.length), 'TYPE').toUpperCase() === 'PART') {
      rememberPlaybackUrl(attribute(line.slice('#EXT-X-PRELOAD-HINT:'.length), 'URI'))
    }
    if (!line.startsWith('#EXT-X-STREAM-INF:')) continue
    const uri = followingUri(lines, index, candidate =>
      candidate.startsWith('#EXT-X-STREAM-INF:') || candidate === '#EXT-X-ENDLIST')
    if (!uri) continue
    let url: string
    try {
      url = inheritManifestAccessQuery(baseUrl, new URL(uri, baseUrl).href)
    } catch {
      continue
    }
    const attributes = line.slice('#EXT-X-STREAM-INF:'.length)
    const resolution = attribute(attributes, 'RESOLUTION').match(/^(\d+)x(\d+)$/i)
    const width = positiveFiniteNumber(resolution?.[1] || '')
    const height = positiveFiniteNumber(resolution?.[2] || '')
    const bandwidth = positiveFiniteNumber(attribute(attributes, 'BANDWIDTH'))
    const codecs = attribute(attributes, 'CODECS') || undefined
    variants.push({
      url,
      width,
      height,
      bandwidth,
      codecs,
      quality: height ? `${height}p` : undefined,
    })
  }
  const mediaPlaylist = completeSegments > 0 || partialSegments > 0
  const isLive = mediaPlaylist ? !lines.some(line => line === '#EXT-X-ENDLIST') : undefined
  const lowLatencyLive = isLive === true && lines.some(line =>
    line.startsWith('#EXT-X-PART:')
      || line.startsWith('#EXT-X-PART-INF:')
      || line.startsWith('#EXT-X-PRELOAD-HINT:')
      || line.startsWith('#EXT-X-SERVER-CONTROL:'),
  )
  const partOnlyLive = isLive === true && partialSegments > 0 && completeSegments === 0
  return {
    variants,
    renditionUrls: renditionUrls.slice(0, 24),
    // The tail of a live window is what the player is currently appending.
    // Bounding this also keeps session storage small on long event playlists.
    playbackUrls: playbackUrls.slice(-24),
    duration: completeDuration && duration > 0 ? duration : undefined,
    isLive,
    lowLatencyLive,
    partOnlyLive,
  }
}

export function resourceQuality(url: string, height?: number): string {
  if (height) return `${height}p`
  const value = url.match(/(?:^|[\/_-])(2160|1440|1080|720|540|480|360|240)p?(?:[\/_?.-]|$)/i)?.[1]
  return value ? `${value}p` : ''
}
