import { replayableRequestHeaders } from './resources'

export type ManifestFetcher = (url: string, init: RequestInit) => Promise<Response>

interface ManifestRequestContext {
  requestHeaders?: Record<string, string>
  /** Supplied by the background only after associating a resource with its page. */
  inspectionHeaders?: Record<string, string>
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

/** Retry a contextual inspection once with replay-safe headers under one deadline. */
export function createManifestFetcher(
  context: ManifestRequestContext,
  fetcher: ManifestFetcher = (url, init) => fetch(url, init),
): (url: string) => Promise<Response> {
  const headers = inspectionHeaders(context.requestHeaders, context.inspectionHeaders)
  const fallbackHeaders = inspectionHeaders(context.requestHeaders)
  const hasContext = Object.keys(context.inspectionHeaders || {}).length > 0
  return async url => {
    const init = { credentials: 'include' as const, headers, signal: AbortSignal.timeout(5_000) }
    let response: Response
    try {
      response = await fetcher(url, init)
    } catch (error) {
      if (!hasContext) throw error
      return fetcher(url, { ...init, headers: fallbackHeaders })
    }
    if (response.ok || !hasContext) return response
    // Release a rejected streaming response before opening its replacement.
    try { await response.body?.cancel() } catch {}
    return fetcher(url, { ...init, headers: fallbackHeaders })
  }
}

/** Estimate complete VOD bytes only when both metadata values are usable. */
export function estimatedManifestBytes(duration: number | undefined, bandwidth: number | undefined): number | undefined {
  if (!Number.isFinite(duration) || (duration || 0) <= 0
    || !Number.isFinite(bandwidth) || (bandwidth || 0) <= 0) return undefined
  const bytes = (duration || 0) * (bandwidth || 0) / 8
  return Number.isFinite(bytes) && bytes > 0 ? Math.round(bytes) : undefined
}
