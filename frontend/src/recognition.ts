export interface RecognitionResult {
  kind: 'hls' | 'dash' | 'file' | 'page' | 'none'
  candidates: Array<{
    url: string
    source?: string
    label?: string
    quality?: string | null
    confidence?: number
  }>
  message?: string
}

export interface RecognitionCandidateView {
  url: string
  source?: string
  label?: string
  quality?: string | null
  confidence?: number
  host: string
  filename: string
  qualityLabel: string
  sourceLabel: string
  recommended: boolean
}

export interface RecognitionView {
  mode: 'ready' | 'choose' | 'not-found'
  message: string
}

export function recognitionView(result: RecognitionResult): RecognitionView {
  if (result.candidates.length === 1) {
    return { mode: 'ready', message: result.message || '' }
  }
  if (result.candidates.length > 1) {
    return { mode: 'choose', message: result.message || '' }
  }
  return { mode: 'not-found', message: result.message || '未发现可下载的 HLS 链接' }
}

const QUALITY_HEIGHTS = [4320, 2160, 1440, 1080, 900, 720, 576, 540, 480, 432, 360, 288, 240, 144]
const QUALITY_QUERY_KEY = /(quality|resolution|height|width|size|rendition|profile|level|bitrate)/i

function decoded(value: string) {
  try {
    return decodeURIComponent(value)
  } catch {
    return value
  }
}

function candidateParts(value: string) {
  try {
    const parsed = new URL(value)
    const pathname = decoded(parsed.pathname)
    const qualityParameters = [...parsed.searchParams]
      .filter(([key]) => QUALITY_QUERY_KEY.test(key))
      .map(([key, parameter]) => `${key}=${decoded(parameter)}`)
      .join(' ')
    return {
      host: parsed.host || '来源未知',
      pathname,
      qualityText: `${pathname} ${qualityParameters}`,
    }
  } catch {
    const pathname = decoded(value.split(/[?#]/, 1)[0])
    return { host: '来源未知', pathname, qualityText: pathname }
  }
}

function candidateFilename(pathname: string) {
  const parts = pathname.split('/').filter(Boolean)
  const filename = parts[parts.length - 1]?.trim()
  return filename || '播放清单'
}

function inferredHeight(text: string) {
  const dimensions = [...text.matchAll(/(?:^|[^\d])(\d{3,4})\s*[x×]\s*(\d{3,4})(?=$|[^\d])/gi)]
    .map(match => Math.min(Number(match[1]), Number(match[2])))
    .filter(height => height >= 144 && height <= 4320)
  if (dimensions.length) return Math.max(...dimensions)

  const heightPattern = new RegExp(`(?:^|[/_.?&=+\\-])(${QUALITY_HEIGHTS.join('|')})(?:p|i)?(?=$|[/_.?&=+\\-])`, 'i')
  const height = text.match(heightPattern)?.[1]
  if (height) return Number(height)

  if (/(?:^|[/_.?&=+\-])(8k)(?=$|[/_.?&=+\-])/i.test(text)) return 4320
  if (/(?:^|[/_.?&=+\-])(4k|uhd)(?=$|[/_.?&=+\-])/i.test(text)) return 2160
  if (/(?:^|[/_.?&=+\-])(2k|qhd)(?=$|[/_.?&=+\-])/i.test(text)) return 1440
  if (/(?:^|[/_.?&=+\-])(fhd)(?=$|[/_.?&=+\-])/i.test(text)) return 1080
  if (/(?:^|[/_.?&=+\-])(hd)(?=$|[/_.?&=+\-])/i.test(text)) return 720
  return 0
}

function isAdaptivePlaylist(text: string) {
  return /(?:^|[/_.?&=+\-])(master|manifest)(?=$|[/_.?&=+\-])/i.test(text)
}

function metadataHeight(quality?: string | null) {
  const value = quality?.match(/^(\d{3,4})p$/i)?.[1]
  return value ? Number(value) : 0
}

function metadataBitrate(quality?: string | null) {
  const value = quality?.match(/^(\d{2,6})\s*kbps$/i)?.[1]
  return value ? Number(value) : 0
}

function candidateScore(qualityText: string, height: number, quality?: string | null, confidence?: number) {
  let score = height + Math.min(metadataBitrate(quality) / 10, 900)
  if (quality === 'master' || quality === 'dash' || isAdaptivePlaylist(qualityText)) score += 10_000
  if (/\.(?:m3u8|mpd)(?:$|[/?#])/i.test(qualityText)) score += 10
  if (/(?:^|[/_.?&=+\-])(audio|aac|subtitle|subtitles|caption|captions|preview|thumbnail|sprite|advert|ads)(?=$|[/_.?&=+\-])/i.test(qualityText)) score -= 100_000
  if (Number.isFinite(confidence)) score += Math.max(0, Math.min(1, confidence || 0))
  return score
}

function qualityLabel(quality: string | null | undefined, height: number, adaptive: boolean) {
  if (quality === 'master' || quality === 'dash' || adaptive) return '自适应清晰度'
  if (/^\d{3,4}p$/i.test(quality || '')) return `推测 ${quality}`
  const bitrate = quality?.match(/^(\d{2,6})\s*kbps$/i)?.[1]
  if (bitrate) return `推测 ${bitrate} kbps`
  return height ? `推测 ${height}p` : '清晰度未知'
}

function sourceLabel(source?: string) {
  if (source === 'file') return '文件直链'
  if (source === 'playlist') return 'HLS 播放清单'
  if (source === 'dash') return 'DASH 播放清单'
  if (source === 'html') return '网页内发现'
  return '已识别资源'
}

/**
 * Converts API candidates into concise, ranked UI rows without changing the
 * recognition response shape. The strongest candidate is moved to the top and
 * marked as the default recommendation; ties retain their API order.
 */
export function recognitionCandidateViews(candidates: RecognitionResult['candidates']): RecognitionCandidateView[] {
  const ranked = candidates.map((candidate, index) => {
    const parts = candidateParts(candidate.url)
    const height = metadataHeight(candidate.quality) || inferredHeight(parts.qualityText)
    const adaptive = candidate.quality === 'master' || isAdaptivePlaylist(parts.qualityText)
    return {
      ...candidate,
      host: parts.host,
      filename: candidateFilename(parts.pathname),
      qualityLabel: qualityLabel(candidate.quality, height, adaptive),
      sourceLabel: sourceLabel(candidate.source),
      score: candidateScore(parts.qualityText, height, candidate.quality, candidate.confidence),
      index,
    }
  })

  ranked.sort((left, right) => right.score - left.score || left.index - right.index)
  return ranked.map(({ score: _score, index: _index, ...candidate }, index) => ({
    ...candidate,
    recommended: index === 0,
  }))
}
