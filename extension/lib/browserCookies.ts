import { getDomain } from 'tldts'

export function cookieLookupUrl(value = ''): string {
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : ''
  } catch {
    return ''
  }
}

export interface ReplayCookie {
  name: string
  value: string
  domain: string
  path: string
  hostOnly: boolean
  secure: boolean
  expirationDate?: number
  partitionKey?: { topLevelSite?: string, hasCrossSiteAncestor?: boolean }
}

export function replayCookieIdentity(cookie: ReplayCookie): string {
  // 普通 Cookie 与当前页面分区 Cookie 可以同名、同域、同 Path，不能互相覆盖。
  return JSON.stringify([cookie.domain, cookie.path, cookie.name,
    cookie.partitionKey?.topLevelSite || '', cookie.partitionKey?.hasCrossSiteAncestor ?? false])
}

export function cookiePartitionSite(value: string): string {
  const target = cookieLookupUrl(value)
  if (!target) return ''
  const { protocol, hostname } = new URL(target)
  // 分区键按 schemeful site 隔离；子域、端口及私有后缀都不能按 origin 猜测。
  const domain = getDomain(hostname, { allowPrivateDomains: true, extractHostname: false }) || hostname
  return `${protocol}//${domain}`
}

export interface SrcdocFrameContext {
  href: string
  origin: string
  ancestorOrigins: string[]
}

export function isCookieHostPermissionError(error: unknown): boolean {
  const detail = error instanceof Error ? error.message : String(error || '')
  return /No host permissions for cookies at url:\s*["'][^"']+["']/i.test(detail)
}

export function isSrcdocCookieHostPermissionError(error: unknown): boolean {
  const detail = error instanceof Error ? error.message : String(error || '')
  return isCookieHostPermissionError(error)
    && /No host permissions for cookies at url:\s*["']about:srcdoc["']/i.test(detail)
}

export function cookiePartitionKeyForSrcdoc(
  topLevelSite: string,
  context: SrcdocFrameContext,
): { topLevelSite: string, hasCrossSiteAncestor: boolean } | undefined {
  const topSite = cookiePartitionSite(topLevelSite)
  if (!topSite || context.href.split(/[?#]/, 1)[0] !== 'about:srcdoc') return undefined

  // srcdoc can have an opaque origin, and referrer policy can redact any
  // ancestor. In either case the browser's ancestor-chain bit is unknowable.
  if (!cookiePartitionSite(context.origin) || !context.ancestorOrigins.length
    || context.ancestorOrigins.some(origin => !cookiePartitionSite(origin))) return undefined

  const ancestorSites = context.ancestorOrigins.map(cookiePartitionSite)
  if (ancestorSites[ancestorSites.length - 1] !== topSite
    || ancestorSites[0] !== cookiePartitionSite(context.origin)) return undefined
  return {
    topLevelSite: topSite,
    hasCrossSiteAncestor: ancestorSites.some(site => site !== topSite),
  }
}

export function cookieMatchesUrl(cookie: ReplayCookie, value: string, now = Date.now() / 1000): boolean {
  const target = cookieLookupUrl(value)
  if (!target) return false
  const url = new URL(target)
  const domain = cookie.domain.replace(/^\./, '').toLowerCase()
  const host = url.hostname.toLowerCase()
  const path = cookie.path || '/'
  return Boolean(domain) && (host === domain || (!cookie.hostOnly && host.endsWith(`.${domain}`)))
    && (!cookie.secure || url.protocol === 'https:')
    && (cookie.expirationDate === undefined || cookie.expirationDate > now)
    && (url.pathname === path || (url.pathname.startsWith(path)
      && (path.endsWith('/') || url.pathname[path.length] === '/')))
    && !/[\r\n\0;]/.test(cookie.name + cookie.value)
}

export function cookieHeaderForUrl(cookies: ReplayCookie[], url: string): string {
  return cookies.filter(cookie => cookieMatchesUrl(cookie, url))
    .sort((a, b) => b.path.length - a.path.length)
    .map(cookie => `${cookie.name}=${cookie.value}`).join('; ')
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
