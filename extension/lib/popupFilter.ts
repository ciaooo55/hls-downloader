import type { MediaResource } from './resources'

export type ResourceFilterCategory = 'all' | 'stream' | 'video' | 'audio' | 'file'

const AUDIO_EXT = /\.(?:aac|flac|m4a|mp3|oga|ogg|opus|wav)$/i
const VIDEO_EXT = /\.(?:avi|m4v|mkv|mov|mp4|webm)$/i

function pathnameSuffix(url: string): string {
  try {
    const match = new URL(url).pathname.match(/(\.[a-z0-9]{1,10})$/i)
    return match?.[1]?.toLowerCase() || ''
  } catch {
    return ''
  }
}

export function classifyResourceCategory(item: Pick<MediaResource, 'kind' | 'mimeType' | 'url'>): Exclude<ResourceFilterCategory, 'all'> {
  if (item.kind === 'hls' || item.kind === 'dash') return 'stream'
  if (item.kind === 'file' || item.kind === 'magnet') return 'file'
  const mime = (item.mimeType || '').toLowerCase()
  const suffix = pathnameSuffix(item.url)
  if (mime.startsWith('audio/') || AUDIO_EXT.test(suffix)) return 'audio'
  if (mime.startsWith('video/') || VIDEO_EXT.test(suffix)) return 'video'
  return 'video'
}

export function filterPopupResources(
  items: MediaResource[],
  category: ResourceFilterCategory,
  query: string,
): MediaResource[] {
  const needle = query.trim().toLowerCase()
  return items.filter(item => {
    if (category !== 'all' && classifyResourceCategory(item) !== category) return false
    if (!needle) return true
    let host = ''
    try { host = new URL(item.url).host } catch {}
    const haystack = [
      item.title,
      item.filename,
      item.url,
      item.quality,
      item.kind,
      host,
    ]
      .filter(Boolean)
      .join(' ')
      .toLowerCase()
    return haystack.includes(needle)
  })
}

export function countPopupCategories(items: MediaResource[]): Record<ResourceFilterCategory, number> {
  const counts: Record<ResourceFilterCategory, number> = {
    all: items.length,
    stream: 0,
    video: 0,
    audio: 0,
    file: 0,
  }
  for (const item of items) {
    counts[classifyResourceCategory(item)] += 1
  }
  return counts
}
