import { parseUrlList } from './urlList'

export const DROP_LINK_SUFFIXES = ['.url', '.magnet', '.m3u', '.m3u8', '.mpd', '.html', '.htm', '.metalink', '.meta4'] as const

export type DropFileKind = 'torrent' | 'link'

export type DropPlan =
  | { kind: 'none' }
  | { kind: 'recognize'; url: string }
  | { kind: 'batch'; urls: string[]; truncated: boolean }
  | { kind: 'files'; items: Array<{ kind: DropFileKind; name: string; path: string }> }

export function classifyDroppedFilename(name: string): DropFileKind | null {
  const lower = String(name || '').trim().replace(/\\/g, '/').split('/').pop()?.toLowerCase() || ''
  if (lower.endsWith('.torrent')) return 'torrent'
  if (DROP_LINK_SUFFIXES.some(suffix => lower.endsWith(suffix))) return 'link'
  return null
}

export function isInternalDropUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    const host = (parsed.hostname || '').toLowerCase()
    return host === '127.0.0.1' || host === 'localhost' || host === '::1'
  } catch {
    return true
  }
}

export function planDroppedPayload(input: { text?: string; files?: Array<{ name?: string; path?: string }> }): DropPlan {
  const items: Array<{ kind: DropFileKind; name: string; path: string }> = []
  for (const file of input.files || []) {
    const name = String(file.path || file.name || '')
    const kind = classifyDroppedFilename(name)
    if (!kind) continue
    items.push({ kind, name, path: String(file.path || '') })
    if (items.length >= 20) break
  }
  if (items.length) return { kind: 'files', items }

  const parsed = parseUrlList(String(input.text || ''))
  const urls = parsed.urls.filter(url => !isInternalDropUrl(url))
  if (!urls.length) return { kind: 'none' }
  if (urls.length === 1) return { kind: 'recognize', url: urls[0] }
  return { kind: 'batch', urls, truncated: parsed.truncated }
}

export function isEditableDropTarget(target: EventTarget | null): boolean {
  const element = target instanceof HTMLElement ? target : null
  if (!element) return false
  const tag = element.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || element.isContentEditable
}
export function payloadFromDataTransfer(data: DataTransfer | null): { text: string; files: Array<{ name: string; path: string }> } {
  if (!data) return { text: '', files: [] }
  const files = Array.from(data.files || []).map(file => ({
    name: file.name,
    path: String((file as File & { path?: string }).path || ''),
  }))
  const text = data.getData('text/uri-list') || data.getData('text/plain') || ''
  return { text, files }
}
