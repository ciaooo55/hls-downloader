export interface HeaderLike {
  name?: string
  value?: string
}

export interface RequestBodyLike {
  formData?: Record<string, string[]>
  raw?: Array<{ bytes?: ArrayBufferLike | Uint8Array }>
}

export interface RequestDetailsLike {
  requestId: string
  url: string
  tabId: number
  frameId?: number
  type?: string
  method?: string
  initiator?: string
  documentUrl?: string
  timeStamp?: number
  requestHeaders?: HeaderLike[]
  requestBody?: RequestBodyLike
  responseHeaders?: HeaderLike[]
  statusCode?: number
  redirectUrl?: string
}

export interface RequestChain {
  requestId: string
  tabId: number
  frameId: number
  type: string
  method: string
  initialUrl: string
  finalUrl: string
  urls: string[]
  pageUrl: string
  requestHeaders: Record<string, string>
  /** Base64 body held in memory only; it is never written to extension storage. */
  requestBody: string
  responseHeaders: Record<string, string>
  statusCode: number
  startedAt: number
  updatedAt: number
}

const MAX_REPLAY_BODY_BYTES = 128 * 1024
const REPLAYABLE_POST_CONTENT_TYPES = new Set([
  'application/json',
  'application/x-www-form-urlencoded',
])

export interface DownloadLike {
  url: string
  finalUrl?: string
  referrer?: string
}

function headers(values: HeaderLike[] | undefined): Record<string, string> {
  const result: Record<string, string> = {}
  for (const header of values || []) {
    const name = String(header.name || '').toLowerCase()
    if (name && header.value !== undefined) result[name] = String(header.value)
  }
  return result
}

function base64(bytes: Uint8Array): string {
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 0x4000) {
    const chunk = bytes.subarray(offset, Math.min(bytes.length, offset + 0x4000))
    for (const value of chunk) binary += String.fromCharCode(value)
  }
  return btoa(binary)
}

/**
 * Keep a small exact request payload in the worker only. Multipart/file bodies
 * and multi-part raw data are deliberately not reconstructed or replayed.
 */
export function captureReplayableRequestBody(body?: RequestBodyLike): string {
  if (!body) return ''
  const raw = body.raw || []
  if (raw.length === 1 && raw[0]?.bytes) {
    const bytes = raw[0].bytes instanceof Uint8Array
      ? raw[0].bytes
      : new Uint8Array(raw[0].bytes)
    return bytes.length && bytes.length <= MAX_REPLAY_BODY_BYTES ? base64(bytes) : ''
  }
  if (raw.length) return ''
  if (!body.formData) return ''
  const params = new URLSearchParams()
  for (const [name, values] of Object.entries(body.formData)) {
    if (!Array.isArray(values)) return ''
    for (const value of values) params.append(name, String(value))
  }
  const bytes = new TextEncoder().encode(params.toString())
  return bytes.length && bytes.length <= MAX_REPLAY_BODY_BYTES ? base64(bytes) : ''
}

export function replayablePostRequest(chain: RequestChain | undefined): {
  request_method?: 'POST'
  request_body?: string
} {
  const contentType = requestHeader(chain, 'content-type').split(';', 1)[0].trim().toLowerCase()
  if (
    chain?.method.toUpperCase() !== 'POST'
    || !chain.requestBody
    || !REPLAYABLE_POST_CONTENT_TYPES.has(contentType)
  ) return {}
  return { request_method: 'POST', request_body: chain.requestBody }
}

function normalized(value: string): string {
  try {
    const url = new URL(value)
    url.hash = ''
    return url.href
  } catch {
    return value.split('#', 1)[0]
  }
}

function appendUrl(values: string[], value: string | undefined): string[] {
  if (!value) return values
  const key = normalized(value)
  return values.some(item => normalized(item) === key) ? values : [...values, value]
}

export function httpOrigin(value: string): string {
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.origin : ''
  } catch {
    return ''
  }
}

export class RequestChainStore {
  private readonly chains = new Map<string, RequestChain>()

  observeRequest(details: RequestDetailsLike): RequestChain {
    const now = details.timeStamp || Date.now()
    const previous = this.chains.get(details.requestId)
    const urls = appendUrl(previous?.urls || [], details.url)
    const capturedHeaders = headers(details.requestHeaders)
    const capturedBody = captureReplayableRequestBody(details.requestBody)
    const chain: RequestChain = {
      requestId: details.requestId,
      tabId: details.tabId,
      frameId: details.frameId ?? previous?.frameId ?? -1,
      type: details.type || previous?.type || '',
      method: details.method || previous?.method || 'GET',
      initialUrl: previous?.initialUrl || details.url,
      finalUrl: details.url,
      urls,
      pageUrl: details.documentUrl || previous?.pageUrl || details.initiator || '',
      requestHeaders: Object.keys(capturedHeaders).length
        ? capturedHeaders
        : previous?.requestHeaders || {},
      requestBody: capturedBody || previous?.requestBody || '',
      responseHeaders: previous?.responseHeaders || {},
      statusCode: previous?.statusCode || 0,
      startedAt: previous?.startedAt || now,
      updatedAt: now,
    }
    this.chains.set(details.requestId, chain)
    return chain
  }

  observeRedirect(details: RequestDetailsLike): RequestChain {
    const chain = this.observeRequest(details)
    chain.urls = appendUrl(chain.urls, details.redirectUrl)
    if (details.redirectUrl) chain.finalUrl = details.redirectUrl
    if (details.responseHeaders) chain.responseHeaders = headers(details.responseHeaders)
    chain.statusCode = details.statusCode || chain.statusCode
    return chain
  }

  observeResponse(details: RequestDetailsLike): RequestChain {
    const chain = this.observeRequest(details)
    chain.finalUrl = details.url
    chain.urls = appendUrl(chain.urls, details.url)
    chain.responseHeaders = headers(details.responseHeaders)
    chain.statusCode = details.statusCode || 0
    return chain
  }

  find(download: DownloadLike, now = Date.now(), preferredTabId?: number): RequestChain | undefined {
    this.cleanup(now)
    const candidates = [download.url, download.finalUrl]
      .filter((value): value is string => Boolean(value))
      .map(normalized)
    const referrer = download.referrer ? normalized(download.referrer) : ''
    return [...this.chains.values()]
      .filter(chain => chain.urls.some(url => candidates.includes(normalized(url))))
      .filter(chain => preferredTabId === undefined || chain.tabId === preferredTabId)
      .sort((left, right) => {
        const leftPageMatch = referrer && normalized(left.pageUrl) === referrer ? 1 : 0
        const rightPageMatch = referrer && normalized(right.pageUrl) === referrer ? 1 : 0
        return rightPageMatch - leftPageMatch || right.updatedAt - left.updatedAt
      })[0]
  }

  contextsForPage(tabId: number, pageUrl: string, now = Date.now(), limit = 12): RequestChain[] {
    this.cleanup(now)
    const page = normalized(pageUrl)
    const supportedTypes = new Set(['xmlhttprequest', 'media', 'other'])
    const selected = new Map<string, RequestChain>()
    const candidates = [...this.chains.values()]
      .filter(chain => chain.tabId === tabId && supportedTypes.has(chain.type))
      .filter(chain => {
        if (!page) return true
        return normalized(chain.pageUrl) === page
          || normalized(chain.requestHeaders.referer || '') === page
      })
      .sort((left, right) => right.updatedAt - left.updatedAt)
    for (const chain of candidates) {
      const origin = httpOrigin(chain.finalUrl)
      if (!origin || selected.has(origin)) continue
      selected.set(origin, chain)
      if (selected.size >= limit) break
    }
    return [...selected.values()]
  }

  /**
   * Return the browser request that established the source page itself.
   * Resource probes often have no headers of their own (Performance/fetch
   * observation), but the page navigation always carries the identity a site
   * expects: UA, Referer/Origin policy and first-party cookies.
   */
  pageContext(tabId: number, pageUrl: string, now = Date.now()): RequestChain | undefined {
    this.cleanup(now)
    const page = normalized(pageUrl)
    return [...this.chains.values()]
      .filter(chain => chain.tabId === tabId)
      .filter(chain => ['main_frame', 'sub_frame', 'xmlhttprequest'].includes(chain.type))
      .filter(chain => !page || normalized(chain.finalUrl) === page || normalized(chain.pageUrl) === page)
      .sort((left, right) => {
        const leftExact = normalized(left.finalUrl) === page ? 1 : 0
        const rightExact = normalized(right.finalUrl) === page ? 1 : 0
        return rightExact - leftExact || right.updatedAt - left.updatedAt
      })[0]
  }

  finish(requestId: string, now = Date.now()): void {
    const chain = this.chains.get(requestId)
    if (chain) chain.updatedAt = now
  }

  cleanup(now = Date.now(), maxAgeMs = 5 * 60_000): void {
    for (const [requestId, chain] of this.chains) {
      if (now - chain.updatedAt > maxAgeMs) this.chains.delete(requestId)
    }
  }
}

export function responseHeader(chain: RequestChain | undefined, name: string): string {
  return chain?.responseHeaders[name.toLowerCase()] || ''
}

export function requestHeader(chain: RequestChain | undefined, name: string): string {
  return chain?.requestHeaders[name.toLowerCase()] || ''
}
