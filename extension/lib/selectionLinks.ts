import { isAuthenticationNavigation, isLikelyDownloadUrl } from './clickIntent'
import { classifyPlaybackSource, classifyResource, pageIdentity } from './resources'

const TEXT_URL = /(?:https?:\/\/|magnet:\?)[^\s<>"']+/gi
const SENTENCE_PUNCTUATION = /[,.;:!?\u3002\uff0c\uff1b\uff1a\uff01\uff1f\u3001]/
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
  let end = value.length
  const counts = new Map<string, number>()
  while (end > 0) {
    const close = value[end - 1]
    if (SENTENCE_PUNCTUATION.test(close)) { end--; continue }
    const open = TRAILING_CLOSERS[close]
    if (!open) break
    if (!counts.size) {
      for (const char of value) counts.set(char, (counts.get(char) || 0) + 1)
    }
    const openCount = counts.get(open) || 0
    const closeCount = counts.get(close) || 0
    // A closing delimiter that balances one inside the URL is part of the URL
    // (`file_(1).zip`, IPv6 `[::1]`, etc.). Strip only unmatched sentence
    // punctuation appended after the selected URL.
    if (closeCount <= openCount) break
    counts.set(close, closeCount - 1)
    end--
  }
  return value.slice(0, end)
}

function normalizeSelectedUrl(value: string, baseUrl: string, textExtraction = false): string {
  const raw = String(value || '').trim()
  const candidate = textExtraction ? trimSelectedUrlPunctuation(raw) : raw
  if (!candidate) return ''
  // URI schemes are case-insensitive, but downstream resource classification
  // uses the canonical lower-case magnet prefix. Plain selected text is not
  // normalized by an HTMLAnchorElement first, so normalize only the scheme
  // while preserving the exact info-hash/query bytes.
  if (/^magnet:\?/i.test(candidate)) return candidate.replace(/^magnet:/i, 'magnet:')
  // `new URL('://broken', base)` treats the malformed value as a relative
  // path. Reject that specific scheme-less form while still allowing normal
  // relative routes whose query happens to contain `https://...`.
  if (candidate.startsWith('://')) return ''
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
  const unique = new Set<string>()
  // The content script keeps at most 40 resources for this selection.
  for (const href of anchorHrefs) {
    const url = normalizeSelectedUrl(href, baseUrl)
    if (url) unique.add(url)
    if (unique.size === 40) return [...unique]
  }
  for (const [value] of selectedText.matchAll(TEXT_URL)) {
    const url = normalizeSelectedUrl(value, baseUrl, true)
    if (url) unique.add(url)
    if (unique.size === 40) break
  }
  return [...unique]
}

export interface PageLinkCandidate {
  href: string
  download?: boolean
}

/**
 * Collect deduplicated downloadable/media links across an entire page in DOM
 * order for the "Download all links on page" context-menu action.
 */
export function pageDownloadUrls(
  anchors: PageLinkCandidate[],
  mediaSrcs: string[],
  baseUrl: string,
  limit = 200,
): string[] {
  const maxItems = Math.max(1, Math.min(200, Math.floor(limit)))
  const basePage = pageIdentity(baseUrl)
  const unique = new Set<string>()
  const remember = (url: string) => {
    if (!url || isAuthenticationNavigation(url)) return
    unique.add(url)
  }

  for (const anchor of anchors) {
    if (unique.size >= maxItems) return [...unique]
    const raw = String(anchor.href || '').trim()
    if (!raw || raw.startsWith('#')) continue
    const url = normalizeSelectedUrl(raw, baseUrl)
    if (!url || isAuthenticationNavigation(url)) continue
    const kind = classifyResource(url)
    if (basePage && pageIdentity(url) === basePage && !anchor.download && !kind) continue
    if (anchor.download || kind !== null || isLikelyDownloadUrl(url)) {
      remember(url)
    }
  }

  for (const src of mediaSrcs) {
    if (unique.size >= maxItems) return [...unique]
    const url = normalizeSelectedUrl(src, baseUrl)
    if (!url || isAuthenticationNavigation(url)) continue
    if (basePage && pageIdentity(url) === basePage && !classifyResource(url)) continue
    if (classifyResource(url) !== null || classifyPlaybackSource(url) !== null) {
      remember(url)
    }
  }

  return [...unique]
}
