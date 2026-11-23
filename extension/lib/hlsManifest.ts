import { inheritManifestAccessQuery } from './urlQuery'
import { resourceFingerprint, resourceId, type MediaAudioTrack } from './resources'

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
  audioTracks: MediaAudioTrack[]
  /** Bounded media/init URLs used to associate an MSE SourceBuffer with this playlist. */
  playbackUrls: string[]
  /** Complete exact ownership without storing every long signed segment URL. */
  playbackUrlHashes: string[]
  duration?: number
  /** Present only for media playlists; a master cannot determine liveness. */
  isLive?: boolean
  /** LL-HLS media playlists advertise partial segments/control directives. */
  lowLatencyLive?: boolean
  /** The current live window contains PART tags but no completed EXTINF segment. */
  partOnlyLive?: boolean
}

function parseAttributes(line: string): Map<string, string> {
  const result = new Map<string, string>()
  let index = 0
  while (index < line.length) {
    while (index < line.length && (line[index] === ',' || /\s/.test(line[index]))) index += 1
    const keyStart = index
    while (index < line.length && /[A-Za-z0-9-]/.test(line[index])) index += 1
    const key = line.slice(keyStart, index).toUpperCase()
    while (index < line.length && /\s/.test(line[index])) index += 1
    if (!key || line[index] !== '=') {
      while (index < line.length && line[index] !== ',') index += 1
      continue
    }
    index += 1
    while (index < line.length && /\s/.test(line[index])) index += 1
    let value = ''
    if (line[index] === '"') {
      index += 1
      const valueStart = index
      while (index < line.length && line[index] !== '"') index += 1
      value = line.slice(valueStart, index)
      if (line[index] === '"') index += 1
      while (index < line.length && line[index] !== ',') index += 1
    } else {
      const valueStart = index
      while (index < line.length && line[index] !== ',') index += 1
      value = line.slice(valueStart, index).trim()
    }
    if (!result.has(key)) result.set(key, value)
  }
  return result
}

function resolveHttpUrl(value: string, baseUrl: string): string {
  if (!value) return ''
  try {
    const parsed = new URL(value, baseUrl)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return ''
    return inheritManifestAccessQuery(baseUrl, parsed.href)
  } catch {
    return ''
  }
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
  const audioTracks: MediaAudioTrack[] = []
  const initUrls: string[] = []
  const playbackUrls: string[] = []
  const rememberPlaybackUrl = (value: string, isInit = false) => {
    const resolved = resolveHttpUrl(value, baseUrl)
    if (!resolved) return
    if (isInit && !initUrls.includes(resolved)) initUrls.push(resolved)
    if (!playbackUrls.includes(resolved)) playbackUrls.push(resolved)
  }
  let duration = 0
  let completeDuration = true
  let completeSegments = 0
  let partialSegments = 0
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]
    if (line.startsWith('#EXT-X-MEDIA:')) {
      const attrs = parseAttributes(line.slice('#EXT-X-MEDIA:'.length))
      const uri = attrs.get('URI') || ''
      let trackUrl: string | undefined
      if (uri) {
        const resolved = resolveHttpUrl(uri, baseUrl)
        if (resolved) {
          trackUrl = resolved
          if (!renditionUrls.includes(resolved)) renditionUrls.push(resolved)
        }
      }
      if ((attrs.get('TYPE') || '').toUpperCase() === 'AUDIO' && audioTracks.length < 24) {
        audioTracks.push({
          label: attrs.get('NAME') || attrs.get('LANGUAGE') || '音轨',
          language: attrs.get('LANGUAGE') || '',
          url: trackUrl,
          default: (attrs.get('DEFAULT') || '').toUpperCase() === 'YES',
        })
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
      rememberPlaybackUrl(parseAttributes(line.slice('#EXT-X-PART:'.length)).get('URI') || '')
    }
    if (line.startsWith('#EXT-X-MAP:')) {
      rememberPlaybackUrl(parseAttributes(line.slice('#EXT-X-MAP:'.length)).get('URI') || '', true)
    }
    if (line.startsWith('#EXT-X-PRELOAD-HINT:')) {
      const attrs = parseAttributes(line.slice('#EXT-X-PRELOAD-HINT:'.length))
      if ((attrs.get('TYPE') || '').toUpperCase() === 'PART') {
        rememberPlaybackUrl(attrs.get('URI') || '')
      }
    }
    if (!line.startsWith('#EXT-X-STREAM-INF:')) continue
    const uri = followingUri(lines, index, candidate =>
      candidate.startsWith('#EXT-X-STREAM-INF:') || candidate === '#EXT-X-ENDLIST')
    if (!uri) continue
    const url = resolveHttpUrl(uri, baseUrl)
    if (!url) continue
    const attrs = parseAttributes(line.slice('#EXT-X-STREAM-INF:'.length))
    const resolution = (attrs.get('RESOLUTION') || '').match(/^(\d+)x(\d+)$/i)
    const width = positiveFiniteNumber(resolution?.[1] || '')
    const height = positiveFiniteNumber(resolution?.[2] || '')
    const bandwidth = positiveFiniteNumber(attrs.get('BANDWIDTH') || '')
    const codecs = attrs.get('CODECS') || undefined
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
  const hashOf = (url: string) => resourceId(resourceFingerprint({ kind: 'hls', url }))
  const allHashes = [...new Set(playbackUrls.map(hashOf))]
  const initHashes = [...new Set(initUrls.map(hashOf))]
  let boundedUrls: string[]
  let boundedHashes: string[]
  if (isLive === false) {
    // VOD starts from init + opening segments; preserve init, head, and tail entries.
    const headUrls = playbackUrls.slice(0, 12)
    const tailUrls = playbackUrls.slice(-12)
    boundedUrls = [...new Set([...initUrls.slice(0, 4), ...headUrls, ...tailUrls])].slice(0, 24)
    const nonInitHashes = allHashes.filter(hash => !initHashes.includes(hash))
    const budget = Math.max(0, 512 - initHashes.length)
    const headCount = Math.ceil(budget / 2)
    const tailCount = budget - headCount
    boundedHashes = [...new Set([
      ...initHashes.slice(0, 512),
      ...nonInitHashes.slice(0, headCount),
      ...(tailCount > 0 ? nonInitHashes.slice(-tailCount) : []),
    ])].slice(0, 512)
  } else {
    // Live playlists play at the window tail, while still needing #EXT-X-MAP init segments.
    const tailBudget = Math.max(0, 24 - Math.min(initUrls.length, 4))
    boundedUrls = [...new Set([...initUrls.slice(0, 4), ...(tailBudget > 0 ? playbackUrls.slice(-tailBudget) : [])])].slice(0, 24)
    const hashTailBudget = Math.max(0, 512 - initHashes.length)
    boundedHashes = [...new Set([
      ...initHashes.slice(0, 512),
      ...(hashTailBudget > 0 ? allHashes.slice(-hashTailBudget) : []),
    ])].slice(0, 512)
  }
  return {
    variants,
    renditionUrls: renditionUrls.slice(0, 24),
    audioTracks,
    playbackUrls: boundedUrls,
    playbackUrlHashes: boundedHashes,
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
