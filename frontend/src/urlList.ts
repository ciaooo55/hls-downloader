export const URL_LIST_LIMIT = 100

const TOKEN_RE = /(?:https?:\/\/[^\s<>"'`]+|ftps?:\/\/[^\s<>"'`]+|sftp:\/\/[^\s<>"'`]+|magnet:\?[^\s<>"'`]+)/gi
const HTML_REF_RE = /\b(?:href|src)=["']([^"']+)["']/gi

function stripTrailingPunctuation(value: string): string {
  return value.replace(/[),.;!?\]>]+$/g, '')
}

function normalizeCapturedUrl(raw: string): string {
  const url = stripTrailingPunctuation(String(raw || '').trim())
  if (!url) return ''
  const lower = url.toLowerCase()
  if (lower.startsWith('magnet:')) {
    return /[?&]xt=/i.test(url) ? url : ''
  }
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:', 'ftp:', 'ftps:', 'sftp:'].includes(parsed.protocol)) return ''
    if (!parsed.hostname) return ''
    return url
  } catch {
    return ''
  }
}

export function parseUrlList(text: string, limit = URL_LIST_LIMIT): { urls: string[]; truncated: boolean } {
  const seen = new Set<string>()
  const urls: string[] = []
  const raw = String(text || '')
  const candidates: string[] = []
  for (const match of raw.matchAll(HTML_REF_RE)) {
    candidates.push(match[1] || '')
  }
  for (const match of raw.matchAll(TOKEN_RE)) {
    candidates.push(match[0] || '')
  }
  for (const item of candidates) {
    const url = normalizeCapturedUrl(item)
    if (!url) continue
    const key = url.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    urls.push(url)
    if (urls.length >= limit) return { urls, truncated: true }
  }
  return { urls, truncated: false }
}

export function formatTaskExport(tasks: Array<{ url?: string; title?: string; filename?: string }>): string {
  const lines: string[] = []
  for (const task of tasks) {
    const url = String(task.url || '').trim()
    if (!url) continue
    const name = String(task.filename || task.title || '').trim()
    if (name) lines.push(`# ${name}`)
    lines.push(url)
  }
  return lines.length ? `${lines.join('\n')}\n` : ''
}

export function downloadTextFile(filename: string, content: string): void {
  const blob = new Blob([content], { type: 'text/plain;charset=utf-8' })
  const objectUrl = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = objectUrl
  link.download = filename
  link.click()
  window.setTimeout(() => URL.revokeObjectURL(objectUrl), 1000)
}
