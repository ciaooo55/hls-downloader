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
      duration += Number(line.slice(8).split(',', 1)[0]) || 0
      completeSegments += 1
      const uri = lines.slice(index + 1).find(value => !value.startsWith('#'))
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
    const uri = lines.slice(index + 1).find(value => !value.startsWith('#'))
    if (!uri) continue
    const attributes = line.slice('#EXT-X-STREAM-INF:'.length)
    const resolution = attribute(attributes, 'RESOLUTION').match(/^(\d+)x(\d+)$/i)
    const width = Number(resolution?.[1] || 0) || undefined
    const height = Number(resolution?.[2] || 0) || undefined
    const bandwidth = Number(attribute(attributes, 'BANDWIDTH')) || undefined
    const codecs = attribute(attributes, 'CODECS') || undefined
    variants.push({
      url: inheritManifestAccessQuery(baseUrl, new URL(uri, baseUrl).href),
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
    duration: duration > 0 ? duration : undefined,
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
