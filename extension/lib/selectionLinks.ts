const TEXT_URL = /(?:https?:\/\/|magnet:\?)[^\s<>"']+/gi
const TRAILING_SENTENCE_PUNCTUATION = /[,.;:!?\u3002\uff0c\uff1b\uff1a\uff01\uff1f\u3001]+$/
const TRAILING_CLOSERS: Record<string, string> = {
  ')': '(',
  ']': '[',
  '}': '{',
  '\u300b': '\u300a',
  '\u3009': '\u3008',
  '\u3011': '\u3010',
  '\u3015': '\u3014',
}

function trimSelectedUrlPunctuation(value: string): string {
  let candidate = value.replace(TRAILING_SENTENCE_PUNCTUATION, '')
  while (candidate) {
    const close = candidate.at(-1) || ''
    const open = TRAILING_CLOSERS[close]
    if (!open) break
    const openCount = [...candidate].filter(char => char === open).length
    const closeCount = [...candidate].filter(char => char === close).length
    // A closing delimiter that balances one inside the URL is part of the URL
    // (`file_(1).zip`, IPv6 `[::1]`, etc.). Strip only unmatched sentence
    // punctuation appended after the selected URL.
    if (closeCount <= openCount) break
    candidate = candidate.slice(0, -1).replace(TRAILING_SENTENCE_PUNCTUATION, '')
  }
  return candidate
}

function normalizeSelectedUrl(value: string, baseUrl: string, textExtraction = false): string {
  const raw = String(value || '').trim()
  const candidate = textExtraction ? trimSelectedUrlPunctuation(raw) : raw
  if (!candidate) return ''
  if (/^magnet:\?/i.test(candidate)) return candidate
  try {
    // Let URL parsing decide whether a candidate is absolute or relative. A
    // relative download route may legitimately contain an absolute URL inside
    // its query (`/download?target=https://cdn/...`). Protocol filtering after
    // resolution still rejects javascript:, data: and other unsafe schemes.
    const resolved = new URL(candidate, baseUrl)
    return resolved.protocol === 'http:' || resolved.protocol === 'https:' ? resolved.href : ''
  } catch {
    return ''
  }
}

/** Return only links covered by the user's current selection, in visual order. */
export function selectedDownloadUrls(anchorHrefs: string[], selectedText: string, baseUrl: string): string[] {
  const values: Array<{ value: string, textExtraction: boolean }> = [
    ...anchorHrefs.map(value => ({ value, textExtraction: false })),
    ...(String(selectedText || '').match(TEXT_URL) || []).map(value => ({ value, textExtraction: true })),
  ]
  const unique = new Set<string>()
  for (const item of values) {
    const normalized = normalizeSelectedUrl(item.value, baseUrl, item.textExtraction)
    if (normalized) unique.add(normalized)
  }
  return [...unique]
}
