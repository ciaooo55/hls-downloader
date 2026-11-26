export const SUBTITLE_SETTINGS_KEY = 'subtitle-settings'
export const SUBTITLE_PAGES_KEY = 'subtitle-pages'
export const SUBTITLE_OFFSETS_KEY = 'subtitle-offsets'
export const SUBTITLE_SESSIONS_KEY = 'subtitle-sessions'
export const SUBTITLE_LANGUAGES = [
  ['zh', '中文'], ['en', '英语'], ['ja', '日语'], ['ko', '韩语'], ['fr', '法语'],
  ['de', '德语'], ['es', '西班牙语'], ['ru', '俄语'], ['pt', '葡萄牙语'],
  ['it', '意大利语'], ['ar', '阿拉伯语'], ['id', '印尼语'], ['vi', '越南语'], ['th', '泰语'],
] as const

export interface SubtitleSettings {
  enabled: boolean
  source: string
  target: string
  offset: number
  showSource: boolean
  prefetch: boolean
  sites: Record<string, boolean>
}
export interface SubtitleCue {
  id: string; start: number; end: number; source: string; translation: string; final: boolean
}
export function subtitlePage(url: string): string {
  try { const value = new URL(url); value.hash = ''; return value.href } catch { return '' }
}
export function subtitleSite(url: string): string {
  try { return new URL(url).hostname.toLowerCase() } catch { return '' }
}
export function subtitleSettings(value: unknown): SubtitleSettings {
  const v = value && typeof value === 'object' ? value as Partial<SubtitleSettings> : {}
  const languages: readonly string[] = SUBTITLE_LANGUAGES.map(([code]) => code)
  const sites: Record<string, boolean> = {}
  if (v.sites && typeof v.sites === 'object') {
    for (const [host, enabled] of Object.entries(v.sites)) {
      if (typeof enabled === 'boolean' && host.length <= 253) sites[host.toLowerCase()] = enabled
    }
  }
  return { enabled: v.enabled === true, source: v.source === 'auto' || languages.includes(v.source || '') ? v.source! : 'auto',
    target: languages.includes(v.target || '') ? v.target! : 'zh', offset: subtitleOffset(v.offset), showSource: v.showSource !== false,
    prefetch: v.prefetch === true, sites }
}
export function subtitleOffset(value: unknown): number {
  const n = Number(value)
  return Number.isFinite(n) ? Math.round(Math.max(-30, Math.min(30, n)) * 10) / 10 : 0
}
export function subtitleAllowed(settings: SubtitleSettings, pages: Record<string, boolean>, url: string): boolean {
  return settings.enabled && (pages[subtitlePage(url)] ?? settings.sites[subtitleSite(url)] ?? true)
}
export function visibleSubtitle(cues: Iterable<SubtitleCue>, time: number, offset: number): SubtitleCue | undefined {
  let result: SubtitleCue | undefined
  for (const cue of cues) {
    if (cue.start + offset <= time && cue.end + offset > time && cue.translation && (!result || cue.start >= result.start)) result = cue
  }
  return result
}

export function subtitleReadyEnd(ranges: readonly (readonly [number, number])[], position: number): number {
  let end = position
  let chained = false
  for (const [start, stop] of [...ranges].sort((a, b) => a[0] - b[0])) {
    if (start > end + (chained ? .02 : .25)) break
    if (stop > end) {
      end = stop
      chained = true
    } else if (stop >= position - .02) {
      chained = true
    }
  }
  return end
}

export interface SubtitleTrackSelectionLike {
  audioIndex: number
  audioLanguage: string
  audioLabel: string
}

export interface SubtitleAudioTrackLike {
  label: string
  language: string
  url?: string
  default?: boolean
}

export function resolveSubtitleAudioSource<T extends { url: string; audioUrl?: string; audioTracks?: SubtitleAudioTrackLike[] }>(
  resource: T,
  selection: SubtitleTrackSelectionLike,
): T {
  const tracks = resource.audioTracks || []
  const pickBest = (candidates: SubtitleAudioTrackLike[]) =>
    candidates.find(t => t.default && t.url)
    || candidates.find(t => Boolean(t.url))
    || candidates.find(t => t.default)
    || candidates[0]

  const groups = new Map<string, SubtitleAudioTrackLike[]>()
  for (const track of tracks) {
    const langKey = (track.language || '').trim().toLowerCase()
    const labelKey = (track.label || '').trim().toLowerCase()
    const key = `${langKey}\0${labelKey}`
    const list = groups.get(key) || []
    list.push(track)
    groups.set(key, list)
  }
  const logicalGroups = [...groups.values()]
  const selLang = (selection.audioLanguage || '').trim().toLowerCase()
  const selLabel = (selection.audioLabel || '').trim().toLowerCase()
  const matchesLang = (track: SubtitleAudioTrackLike) =>
    Boolean(selLang) && (track.language || '').trim().toLowerCase() === selLang
  const matchesLabel = (track: SubtitleAudioTrackLike) =>
    Boolean(selLabel) && (track.label || '').trim().toLowerCase() === selLabel
  let matchedGroups: SubtitleAudioTrackLike[][] = []
  if (selLang && selLabel) {
    matchedGroups = logicalGroups.filter(group => group.some(track => matchesLang(track) && matchesLabel(track)))
    if (matchedGroups.length === 0) {
      matchedGroups = logicalGroups.filter(group => group.some(track => matchesLang(track) || matchesLabel(track)))
    }
  } else if (selLang) {
    matchedGroups = logicalGroups.filter(group => group.some(matchesLang))
  } else if (selLabel) {
    matchedGroups = logicalGroups.filter(group => group.some(matchesLabel))
  }

  const allLanguageFreeRenditions = tracks.length > 1
    && tracks.every(track => !(track.language || '').trim())
    && tracks.filter(track => track.default === true).length === 1

  let chosen: SubtitleAudioTrackLike | undefined
  if (matchedGroups.length === 1) {
    chosen = pickBest(matchedGroups[0])
  } else if (logicalGroups.length === 1 && !selLang && !selLabel) {
    chosen = pickBest(logicalGroups[0])
  } else if (allLanguageFreeRenditions && selection.audioIndex <= 0 && !selLang && !selLabel) {
    chosen = pickBest(tracks)
  }

  if ((logicalGroups.length > 1 && !chosen) || (selection.audioIndex > 0 && !chosen?.url)) {
    throw new Error('无法确认当前备用音轨，请关闭提前翻译以采集播放器实际音轨')
  }
  if (chosen?.url) {
    return { ...resource, url: chosen.url, audioUrl: undefined }
  }
  if (resource.audioUrl) {
    return { ...resource, url: resource.audioUrl, audioUrl: undefined }
  }
  return resource
}
