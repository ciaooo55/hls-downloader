export type HarvestCategory = 'video' | 'audio' | 'archive' | 'document' | 'program' | 'playlist' | 'torrent' | 'other'

export interface HarvestLink {
  url: string
  filename: string
  label: string
  extension: string
  category: HarvestCategory | string
  source: string
  size?: number
}

export interface HarvestResult {
  kind: 'page' | 'file' | 'hls' | 'dash' | 'none'
  page_url: string
  final_url: string
  title: string
  links: HarvestLink[]
  truncated: boolean
  message: string
}

export const HARVEST_FILTERS: Array<{ id: 'all' | HarvestCategory; label: string }> = [
  { id: 'all', label: '全部' },
  { id: 'video', label: '视频' },
  { id: 'audio', label: '音频' },
  { id: 'archive', label: '压缩包' },
  { id: 'document', label: '文档' },
  { id: 'program', label: '程序' },
  { id: 'playlist', label: '清单' },
  { id: 'torrent', label: '种子' },
]

export function filterHarvestLinks(links: HarvestLink[], category: 'all' | HarvestCategory): HarvestLink[] {
  if (category === 'all') return links
  return links.filter(item => item.category === category)
}

export function harvestFilterCounts(links: HarvestLink[]): Record<string, number> {
  const counts: Record<string, number> = { all: links.length }
  for (const item of links) {
    counts[item.category] = (counts[item.category] || 0) + 1
  }
  return counts
}
export interface HarvestProbe {
  url: string
  size?: number | null
  content_type?: string
  ok?: boolean
}

export function applyHarvestProbes(links: HarvestLink[], probes: HarvestProbe[]): HarvestLink[] {
  const sizes = new Map<string, number>()
  for (const probe of probes) {
    if (probe.ok && Number(probe.size) > 0) sizes.set(probe.url, Number(probe.size))
  }
  return links.map(item => sizes.has(item.url) ? { ...item, size: sizes.get(item.url) } : item)
}

export function filterHarvestLinksByMinSize(links: HarvestLink[], minBytes: number): HarvestLink[] {
  if (minBytes <= 0) return links
  return links.filter(item => Number(item.size || 0) >= minBytes)
}
