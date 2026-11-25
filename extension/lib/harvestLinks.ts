import type { MediaResource } from './resources'

export type HarvestLinkCategory = 'all' | 'media' | 'archive' | 'installer' | 'document'

export const HARVEST_CATEGORY_CHIPS: ReadonlyArray< readonly [HarvestLinkCategory, string] > = [
  ['all', '全部'],
  ['media', '视频/音频'],
  ['archive', '压缩包'],
  ['installer', '程序/镜像'],
  ['document', '文档'],
]

const MEDIA_LINK_EXT = /\.(?:m3u8?|mpd|mp4|m4v|webm|mkv|mov|avi|flv|f4v|3gp|ts|m2ts|m4a|mp3|flac|wav|ogg|oga|opus|aac)(?:$|[?#])/i
const ARCHIVE_LINK_EXT = /\.(?:zip|7z|rar|tar|tgz|tbz2|txz|gz|bz2|xz|zst|lz4|cab)(?:$|[?#])/i
const INSTALLER_LINK_EXT = /\.(?:exe|msi|msix|appx|apk|aab|dmg|pkg|deb|rpm|appimage|iso|img|vhd|vhdx|vmdk|qcow2|vsix|nupkg|jar|bin|torrent|metalink|meta4)(?:$|[?#])/i
const DOCUMENT_LINK_EXT = /\.(?:pdf|epub|mobi|azw3|docx?|xlsx?|pptx?|csv|tsv|txt|md|rtf|odt|ods|odp)(?:$|[?#])/i

function resourceNameAndPath(item: Pick<MediaResource, 'url' | 'filename'>): string {
  const filename = String(item.filename || '').trim()
  if (filename && /\.[a-z0-9]{1,10}$/i.test(filename)) return filename
  try {
    return decodeURIComponent(new URL(item.url).pathname)
  } catch {
    return item.url
  }
}

export function classifyHarvestCategory(
  item: Pick<MediaResource, 'kind' | 'url' | 'mimeType' | 'filename'>,
): Exclude<HarvestLinkCategory, 'all'> | 'other' {
  if (item.kind === 'hls' || item.kind === 'dash' || item.kind === 'media') return 'media'
  if (item.kind === 'magnet') return 'installer'
  const mime = String(item.mimeType || '').toLowerCase()
  const target = resourceNameAndPath(item)
  if (mime.startsWith('video/') || mime.startsWith('audio/') || MEDIA_LINK_EXT.test(target)) return 'media'
  if (ARCHIVE_LINK_EXT.test(target) || /(?:zip|7z|rar|tar|gzip|bzip2)/.test(mime)) return 'archive'
  if (INSTALLER_LINK_EXT.test(target) || /(?:iso9660|apple-diskimage|msdownload|msi|debian-package|android\.package-archive|bittorrent)/.test(mime)) {
    return 'installer'
  }
  if (DOCUMENT_LINK_EXT.test(target) || /(?:pdf|epub|msword|ms-excel|ms-powerpoint|officedocument|text\/csv)/.test(mime)) {
    return 'document'
  }
  return 'other'
}

export function countHarvestCategories(items: readonly MediaResource[]): Record<HarvestLinkCategory, number> {
  const counts: Record<HarvestLinkCategory, number> = {
    all: items.length,
    media: 0,
    archive: 0,
    installer: 0,
    document: 0,
  }
  for (const item of items) {
    const bucket = classifyHarvestCategory(item)
    if (bucket !== 'other') counts[bucket] += 1
  }
  return counts
}

export function filterHarvestResources(
  items: readonly MediaResource[],
  category: HarvestLinkCategory,
  query: string,
): MediaResource[] {
  const needle = query.trim().toLowerCase()
  return items.filter(item => {
    if (category !== 'all' && classifyHarvestCategory(item) !== category) return false
    if (!needle) return true
    let host = ''
    let pathname = ''
    try {
      const parsed = new URL(item.url)
      host = parsed.host
      pathname = decodeURIComponent(parsed.pathname)
    } catch {
      pathname = item.url
    }
    const haystack = [
      item.title,
      item.filename,
      pathname,
      host,
      item.kind,
      item.quality,
    ].filter(Boolean).join(' ').toLowerCase()
    return haystack.includes(needle)
  })
}
