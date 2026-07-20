export interface HeaderLike {
  name?: string
  value?: string
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
  responseHeaders: Record<string, string>
  statusCode: number
  startedAt: number
  updatedAt: number
}

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

export class RequestChainStore {
  private readonly chains = new Map<string, RequestChain>()

  observeRequest(details: RequestDetailsLike): RequestChain {
    const now = details.timeStamp || Date.now()
    const previous = this.chains.get(details.requestId)
    const urls = appendUrl(previous?.urls || [], details.url)
    const capturedHeaders = headers(details.requestHeaders)
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

  find(download: DownloadLike, now = Date.now()): RequestChain | undefined {
    this.cleanup(now)
    const candidates = [download.url, download.finalUrl]
      .filter((value): value is string => Boolean(value))
      .map(normalized)
    const referrer = download.referrer ? normalized(download.referrer) : ''
    return [...this.chains.values()]
      .filter(chain => chain.urls.some(url => candidates.includes(normalized(url))))
      .sort((left, right) => {
        const leftPageMatch = referrer && normalized(left.pageUrl) === referrer ? 1 : 0
        const rightPageMatch = referrer && normalized(right.pageUrl) === referrer ? 1 : 0
        return rightPageMatch - leftPageMatch || right.updatedAt - left.updatedAt
      })[0]
  }

  finish(requestId: string, now = Date.now()): void {
    const chain = this.chains.get(requestId)
    if (chain) chain.updatedAt = now
  }

  cleanup(now = Date.now(), maxAgeMs = 30_000): void {
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
