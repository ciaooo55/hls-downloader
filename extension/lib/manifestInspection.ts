import { replayableRequestHeaders } from './resources'

export type ManifestFetcher = (url: string, init: RequestInit) => Promise<Response>

export interface ManifestRequestHeaders {
  requestHeaders?: Record<string, string>
  /** Supplied by the background only after associating a resource with its page. */
  inspectionHeaders?: Record<string, string>
}

export interface ManifestRequestContext extends ManifestRequestHeaders {
  url: string
  /** Aborted when the owning page is replaced or closed, including body reads. */
  signal?: AbortSignal
  /** Return the target's complete identity; this replaces the base headers. */
  requestContextForUrl?: (url: string) => Promise<ManifestRequestHeaders>
}

function inspectionHeaders(
  values: Record<string, string> | undefined,
  contextual: Record<string, string> = {},
): Record<string, string> {
  const headers = replayableRequestHeaders(values)
  for (const name of Object.keys(headers)) {
    if (name === 'user-agent' || name.startsWith('sec-')) delete headers[name]
  }
  // Context can only augment these headers, with the same injection/size bounds
  // for HLS master/rendition requests and DASH requests.
  for (const [rawName, rawValue] of Object.entries(contextual)) {
    const name = String(rawName || '').trim().toLowerCase()
    const value = String(rawValue || '').trim()
    if (!['referer', 'origin', 'cookie'].includes(name) || !value || /[\r\n]/.test(value) || value.length > 32 * 1024) continue
    headers[name] = value
  }
  return headers
}

/** Resolve each rendition's identity before a bounded, best-effort inspection. */
export function createManifestFetcher(
  context: ManifestRequestContext,
  fetcher: ManifestFetcher = (url, init) => fetch(url, init),
): (url: string) => Promise<Response> {
  return async url => {
    context.signal?.throwIfAborted()
    let scoped: ManifestRequestHeaders
    if (context.requestContextForUrl) scoped = await context.requestContextForUrl(url)
    else {
      const sameOrigin = new URL(url).origin === new URL(context.url).origin
      scoped = {
        requestHeaders: sameOrigin ? context.requestHeaders : Object.fromEntries(
          Object.entries(context.requestHeaders || {}).filter(([name]) =>
            ['accept', 'accept-language', 'referer', 'origin'].includes(name.toLowerCase())),
        ),
        inspectionHeaders: Object.fromEntries(Object.entries(context.inspectionHeaders || {})
          .filter(([name]) => name.toLowerCase() !== 'cookie' || url === context.url)),
      }
    }
    context.signal?.throwIfAborted()
    const headers = inspectionHeaders(scoped.requestHeaders, scoped.inspectionHeaders)
    const fallbackHeaders = inspectionHeaders(scoped.requestHeaders)
    const hasContext = JSON.stringify(headers) !== JSON.stringify(fallbackHeaders)
    // Observed URLs are already final. Never forward custom tokens through an
    // unseen redirect; the browser's next observation can identify that target.
    const timeout = AbortSignal.timeout(5_000)
    const signal = context.signal ? AbortSignal.any([context.signal, timeout]) : timeout
    const init = { credentials: 'omit' as const, redirect: 'error' as const, headers, signal }
    let response: Response
    try {
      response = await fetcher(url, init)
    } catch (error) {
      if (!hasContext || signal.aborted) throw error
      response = await fetcher(url, { ...init, headers: fallbackHeaders })
    }
    if (!response.ok) {
      try { await response.body?.cancel() } catch {}
    }
    return response
  }
}

/** Estimate complete VOD bytes only when both metadata values are usable. */
export function estimatedManifestBytes(duration: number | undefined, bandwidth: number | undefined): number | undefined {
  if (!Number.isFinite(duration) || (duration || 0) <= 0
    || !Number.isFinite(bandwidth) || (bandwidth || 0) <= 0) return undefined
  const bytes = (duration || 0) * (bandwidth || 0) / 8
  return Number.isFinite(bytes) && bytes > 0 ? Math.round(bytes) : undefined
}

function isPrivateIpv4(host: string): boolean {
  const parts = host.split('.')
  if (parts.length !== 4) return false
  const nums = parts.map(part => (/^\d+$/.test(part) ? Number(part) : Number.NaN))
  if (nums.some(n => !Number.isInteger(n) || n < 0 || n > 255)) return false
  const [a, b, c] = nums
  return a === 0
    || a === 10
    || a === 127
    || (a === 100 && b >= 64 && b <= 127)
    || (a === 169 && b === 254)
    || (a === 172 && b >= 16 && b <= 31)
    || (a === 192 && b === 0 && c === 0)
    || (a === 192 && b === 168)
    || (a === 198 && (b === 18 || b === 19))
    || a >= 224
}

function parseMappedIpv4FromIpv6(host: string): string | undefined {
  const mapped = host.match(/^::ffff:(?:0:)?(\d+\.\d+\.\d+\.\d+)$/)
  if (mapped) return mapped[1]
  const hexMapped = host.match(/^::ffff:(?:0:)?([0-9a-f]{1,4}):([0-9a-f]{1,4})$/)
  if (!hexMapped) return undefined
  const hi = Number.parseInt(hexMapped[1], 16)
  const lo = Number.parseInt(hexMapped[2], 16)
  return `${(hi >> 8) & 0xff}.${hi & 0xff}.${(lo >> 8) & 0xff}.${lo & 0xff}`
}

export function isPrivateOrLocalHost(hostname: string): boolean {
  const host = String(hostname || '').trim().toLowerCase().replace(/^\[|\]$/g, '').replace(/\.+$/, '')
  if (!host) return true
  if (
    host === 'localhost'
    || host.endsWith('.localhost')
    || host.endsWith('.local')
    || host.endsWith('.internal')
    || host.endsWith('.home.arpa')
  ) return true
  if (!host.includes('.') && !host.includes(':')) return true
  if (host === '::' || host === '::1' || /^0(:0)*:1$/.test(host) || /^0(:0)*$/.test(host)) return true
  if (isPrivateIpv4(host)) return true
  if (host.includes(':')) {
    const mappedIpv4 = parseMappedIpv4FromIpv6(host)
    if (mappedIpv4) return isPrivateIpv4(mappedIpv4)
    if (/^64:ff9b(?::1)?:/.test(host)) return true
    const firstHextet = host.split(':')[0]
    if (
      /^f[cd][0-9a-f]{2}$/.test(firstHextet)
      || /^fe[89ab][0-9a-f]$/.test(firstHextet)
      || /^fe[cdef][0-9a-f]$/.test(firstHextet)
    ) return true
  }
  return false
}

/**
 * Guard background manifest fetches against public-page-to-LAN/loopback SSRF.
 * Local pages (e.g., localhost dev servers) may still inspect local manifests.
 */
export function isSafeInspectionTarget(targetUrl: string, sourceUrl = ''): boolean {
  let target: URL
  try {
    target = new URL(targetUrl)
  } catch {
    return false
  }
  if (target.protocol !== 'http:' && target.protocol !== 'https:') return false
  if (!isPrivateOrLocalHost(target.hostname)) return true
  try {
    const source = new URL(sourceUrl)
    return (source.protocol === 'http:' || source.protocol === 'https:') && isPrivateOrLocalHost(source.hostname)
  } catch {
    return false
  }
}
