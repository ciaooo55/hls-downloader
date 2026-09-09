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
  try {
    const parsed = raw.includes('://') ? new URL(raw) : new URL(`https://${raw}`)
    return parsed.hostname.replace(/^www\./, '').replace(/\.$/, '')
  } catch {
    return raw.replace(/^\*\./, '').replace(/^www\./, '').replace(/:\d+$/, '').replace(/\.$/, '')
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
