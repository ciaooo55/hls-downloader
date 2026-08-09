import { removeRawQueryParameters } from './urlQuery'

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
  /**
   * Browser cancellation/pause can terminate the network request just before
   * downloads.onCreated runs. Keep that exact request identity briefly so the
   * item can still be matched without borrowing another tab's headers.
   */
  failedAt?: number
}

const MAX_REPLAY_BODY_BYTES = 128 * 1024
const MAX_REQUEST_HEADERS = 64
const MAX_HEADER_NAME_LENGTH = 256
const MAX_HEADER_VALUE_LENGTH = 16 * 1024
const MAX_REQUEST_HEADER_BYTES = 32 * 1024
const REPLAYABLE_POST_CONTENT_TYPES = new Set([
  'application/json',
  'application/x-www-form-urlencoded',
])
const ADAPTIVE_MANIFEST_PATH = /\.(?:m3u8?|mpd)$/i
const VOLATILE_MEDIA_QUERY = /^(?:token|auth|authorization|signature|sig|expires?|expiry|policy|key-pair-id|hdnea|hmac|jwt|session|sessionid|access[_-]?key|x-amz-.+)$/i

export interface DownloadLike {
  url: string
  finalUrl?: string
  referrer?: string
}

function headers(values: HeaderLike[] | undefined): Record<string, string> {
  const result: Record<string, string> = {}
  let totalBytes = 0
  for (const header of (values || []).slice(0, MAX_REQUEST_HEADERS)) {
    const name = String(header.name || '').trim().toLowerCase().slice(0, MAX_HEADER_NAME_LENGTH)
    if (!name || header.value === undefined || /[\r\n]/.test(name)) continue
    const value = String(header.value).replace(/[\r\n]/g, '').slice(0, MAX_HEADER_VALUE_LENGTH)
    const bytes = name.length + value.length
    if (totalBytes + bytes > MAX_REQUEST_HEADER_BYTES) break
    totalBytes += bytes
    result[name] = value
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
    const rawBytes = raw[0].bytes
    const byteLength = rawBytes instanceof Uint8Array ? rawBytes.byteLength : Number(rawBytes.byteLength || 0)
    if (!Number.isFinite(byteLength) || byteLength <= 0 || byteLength > MAX_REPLAY_BODY_BYTES) return ''
    const bytes = rawBytes instanceof Uint8Array
      ? rawBytes
      : new Uint8Array(rawBytes)
    return bytes.length && bytes.length <= MAX_REPLAY_BODY_BYTES ? base64(bytes) : ''
  }
  if (raw.length) return ''
  if (!body.formData) return ''
  const params = new URLSearchParams()
  let fieldCount = 0
  let totalChars = 0
  for (const [name, values] of Object.entries(body.formData).slice(0, 128)) {
    if (!Array.isArray(values)) return ''
    if (++fieldCount > 128 || name.length > MAX_HEADER_NAME_LENGTH) return ''
    for (const value of values.slice(0, 128)) {
      const stringValue = String(value)
      totalChars += name.length + stringValue.length
      if (totalChars > MAX_REPLAY_BODY_BYTES * 2) return ''
      params.append(name, stringValue)
    }
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
    // LL-HLS cursors identify one poll, not one stream. Preserve every raw
    // byte of the surrounding signed query while removing only those fields.
    const canonical = removeRawQueryParameters(
      value,
      new Set(['_hls_msn', '_hls_part', '_hls_skip']),
    )
    const hashAt = canonical.indexOf('#')
    return hashAt >= 0 ? canonical.slice(0, hashAt) : canonical
  } catch {
    return value.split('#', 1)[0]
  }
}

function mediaRequestKey(value: string): string {
  const canonical = normalized(value)
  try {
    const url = new URL(canonical)
    const names = new Set([...url.searchParams.keys()].map(key => key.toLowerCase()))
    const terseSignature = names.has('s') && names.has('e')
    const hasVolatileCredential = [...url.searchParams.keys()].some(key => VOLATILE_MEDIA_QUERY.test(key))
    // Signed MP4/archive URLs rotate exactly like adaptive manifests. Keep
    // meaningful selectors (quality, language, asset id) but ignore only known
    // credential fields so a Performance-observed URL can be rebound to the
    // browser's latest successful request and headers.
    if (!ADAPTIVE_MANIFEST_PATH.test(url.pathname) && !terseSignature && !hasVolatileCredential) return canonical
    for (const key of [...url.searchParams.keys()]) {
      if (VOLATILE_MEDIA_QUERY.test(key) || (terseSignature && ['s', 'e', '_t'].includes(key.toLowerCase()))) {
        url.searchParams.delete(key)
      }
    }
    url.searchParams.sort()
    return url.href
  } catch {
    return canonical
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

  constructor(private readonly maxEntries = 1_500) {}

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
      failedAt: 0,
    }
    this.chains.set(details.requestId, chain)
    this.trim()
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

  find(download: DownloadLike, now = Date.now(), preferredTabId?: number, successfulOnly = false): RequestChain | undefined {
    this.cleanup(now)
    const candidates = [download.url, download.finalUrl]
      .filter((value): value is string => Boolean(value))
      .map(mediaRequestKey)
    const referrer = download.referrer ? normalized(download.referrer) : ''
    return [...this.chains.values()]
      .filter(chain => chain.urls.some(url => candidates.includes(mediaRequestKey(url))))
      .filter(chain => !successfulOnly || (chain.statusCode >= 200 && chain.statusCode < 400))
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

  fail(requestId: string, now = Date.now()): void {
    const chain = this.chains.get(requestId)
    if (!chain) return
    // Chrome/Edge may report ERR_ABORTED when a just-created DownloadItem is
    // paused by the takeover path. Deleting the chain here races onCreated and
    // loses the final URL, tab and authenticated headers.
    chain.failedAt = now
    chain.updatedAt = now
  }

  clearTab(tabId: number): void {
    for (const [requestId, chain] of this.chains) {
      if (chain.tabId === tabId) this.chains.delete(requestId)
    }
  }

  private trim(): void {
    if (this.chains.size <= this.maxEntries) return
    // Evict a batch rather than sorting the whole map for every subsequent
    // HLS segment. The newest 80% remains available for delayed user clicks.
    const target = Math.max(1, Math.floor(this.maxEntries * 0.8))
    const remove = this.chains.size - target
    const oldest = [...this.chains.values()]
      .sort((left, right) => left.updatedAt - right.updatedAt)
      .slice(0, remove)
    for (const chain of oldest) this.chains.delete(chain.requestId)
  }

  cleanup(now = Date.now(), maxAgeMs = 5 * 60_000): void {
    for (const [requestId, chain] of this.chains) {
      const retention = chain.failedAt ? Math.min(maxAgeMs, 20_000) : maxAgeMs
      if (now - chain.updatedAt > retention) this.chains.delete(requestId)
    }
  }
}

export function responseHeader(chain: RequestChain | undefined, name: string): string {
  return chain?.responseHeaders[name.toLowerCase()] || ''
}

export function requestHeader(chain: RequestChain | undefined, name: string): string {
  return chain?.requestHeaders[name.toLowerCase()] || ''
}
