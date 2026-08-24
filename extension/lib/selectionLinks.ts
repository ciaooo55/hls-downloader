const TEXT_URL = /(?:https?:\/\/|magnet:\?)[^\s<>"']+/gi
const TRAILING_PUNCTUATION = /[),.;:!?\]}\u3002\uff0c\uff1b\uff1a\uff01\uff1f\u3001\u300b\u3009\u3011\u3015]+$/

function normalizeSelectedUrl(value: string, baseUrl: string): string {
  const candidate = String(value || '').trim().replace(TRAILING_PUNCTUATION, '')
  if (!candidate) return ''
  if (/^magnet:\?/i.test(candidate)) return candidate
  if (candidate.includes('://') && !/^https?:\/\//i.test(candidate)) return ''
  try {
    const resolved = new URL(candidate, baseUrl)
    return resolved.protocol === 'http:' || resolved.protocol === 'https:' ? resolved.href : ''
  } catch {
    return ''
  }
}

/** Return only links covered by the user's current selection, in visual order. */
export function selectedDownloadUrls(anchorHrefs: string[], selectedText: string, baseUrl: string): string[] {
  const values = [
    ...anchorHrefs,
    ...(String(selectedText || '').match(TEXT_URL) || []),
  ]
  const unique = new Set<string>()
  for (const value of values) {
    const normalized = normalizeSelectedUrl(value, baseUrl)
    if (normalized) unique.add(normalized)
  }
  return [...unique]
}
