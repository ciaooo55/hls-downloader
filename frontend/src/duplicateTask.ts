export type DuplicateSuggestedAction = 'resume' | 'retry' | 'start' | 'open' | 'focus' | 'none'

export type DuplicateMatch = {
  id: string
  status: string
  filename?: string
  output_path?: string
  suggested_action?: DuplicateSuggestedAction
  available_actions?: string[]
  output_missing?: boolean
}

export type DuplicatePrompt = {
  message: string
  duplicates: DuplicateMatch[]
}

const ACTION_LABEL: Record<DuplicateSuggestedAction, string> = {
  resume: '\u7ee7\u7eed\u5df2\u6709\u4efb\u52a1',
  retry: '\u91cd\u8bd5\u5df2\u6709\u4efb\u52a1',
  start: '\u5f00\u59cb\u5df2\u6709\u4efb\u52a1',
  open: '\u6253\u5f00\u5df2\u6709\u6587\u4ef6',
  focus: '\u67e5\u770b\u5df2\u6709\u4efb\u52a1',
  none: '',
}

export function parseDuplicateMatches(detail: unknown): DuplicateMatch[] {
  const raw = detail && typeof detail === 'object' ? (detail as { duplicates?: unknown }).duplicates : null
  if (!Array.isArray(raw)) return []
  const matches: DuplicateMatch[] = []
  for (const item of raw) {
    if (!item || typeof item !== 'object') continue
    const row = item as Record<string, unknown>
    const id = String(row.id || '').trim()
    if (!id) continue
    const suggested = String(row.suggested_action || 'none')
    matches.push({
      id,
      status: String(row.status || ''),
      filename: String(row.filename || ''),
      output_path: String(row.output_path || ''),
      suggested_action: ['resume', 'retry', 'start', 'open', 'focus'].includes(suggested)
        ? suggested as DuplicateSuggestedAction
        : 'none',
      available_actions: Array.isArray(row.available_actions) ? row.available_actions.map((value) => String(value)) : [],
      output_missing: Boolean(row.output_missing),
    })
  }
  return matches
}

export function parseDuplicateError(error: unknown): DuplicatePrompt {
  const detail = error && typeof error === 'object' ? (error as { detail?: unknown; message?: string }).detail : null
  const message = (
    detail && typeof detail === 'object' && typeof (detail as { message?: string }).message === 'string'
      ? (detail as { message: string }).message
      : error instanceof Error ? error.message : '\u4e0b\u8f7d\u5217\u8868\u4e2d\u5df2\u6709\u76f8\u540c\u94fe\u63a5'
  )
  return { message, duplicates: parseDuplicateMatches(detail && typeof detail === 'object' ? detail : error) }
}

export function primaryDuplicate(matches: DuplicateMatch[]): DuplicateMatch | null {
  return matches[0] || null
}

export function duplicateActionLabel(action: DuplicateSuggestedAction | undefined): string {
  return ACTION_LABEL[action || 'none'] || ''
}
