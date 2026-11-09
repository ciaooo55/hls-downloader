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
    const init = { credentials: 'include' as const, redirect: 'error' as const, headers, signal }
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
