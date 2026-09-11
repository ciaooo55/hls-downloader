export function cookieLookupUrl(value = ''): string {
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : ''
  } catch {
    return ''
  }
}

export function cookiePermissionHost(value = ''): string {
  const raw = String(value || '').trim().toLowerCase()
  if (!raw) return ''
  const normalize = (host: string) => host
    .replace(/^\*\./, '')
    .replace(/^www\./, '')
    .replace(/\.$/, '')
  try {
    const parsed = raw.includes('://') ? new URL(raw) : new URL(`https://${raw}`)
    return normalize(parsed.hostname)
  } catch {
    return normalize(raw.replace(/:\d+$/, ''))
  }
}

export function normalizeCookiePermissionHosts(values: unknown): string[] {
  if (!Array.isArray(values)) return []
  return [...new Set(values.map(value => cookiePermissionHost(String(value || ''))).filter(Boolean))].slice(0, 256)
}

export function cookiePermissionAllows(
  resourceUrl: string,
  pageUrl: string,
  authorizedHosts: unknown,
): boolean {
  const resourceHost = cookiePermissionHost(resourceUrl)
  const pageHost = cookiePermissionHost(pageUrl)
  const rules = normalizeCookiePermissionHosts(authorizedHosts)
  return rules.some(rule => [resourceHost, pageHost].some(host => (
    Boolean(host) && (host === rule || host.endsWith(`.${rule}`))
  )))
}
