const REDACTED = '<redacted>'

export function redactUrlForDiagnostics(value = ''): string {
  const raw = String(value || '').trim()
  if (!raw) return ''
  try {
    const url = new URL(raw)
    if (!['http:', 'https:'].includes(url.protocol)) {
      return `${url.protocol}${url.search ? '?<redacted>' : ''}`
    }
    url.username = ''
    url.password = ''
    url.hash = ''
    const keys = [...new Set(url.searchParams.keys())]
    url.search = ''
    keys.forEach(key => url.searchParams.append(key, REDACTED))
    return url.toString()
  } catch {
    return '<invalid-or-non-url>'
  }
}
