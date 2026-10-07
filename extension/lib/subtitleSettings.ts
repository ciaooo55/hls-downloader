export const SUBTITLE_SETTINGS_KEY = 'subtitle-settings'
export const SUBTITLE_PAGES_KEY = 'subtitle-pages'
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
