import { isAuthenticationNavigation } from './clickIntent'
import { isConcreteDownloadMime, looksLikeDownloadFile, type ResourceKind } from './resources'

/** Historical reference constant; `minimumBytes` defaults to `0` so Core accelerates or streams HTTP(S) GET directly. */
export const ACCELERATED_TAKEOVER_MIN_BYTES = 1 * 1024 * 1024

export interface AcceleratedTakeoverInput {
  kind?: ResourceKind | null
  url?: string
  method?: string
  statusCode?: number
  responseHeaders?: Record<string, string>
  itemState?: string
  itemTotalBytes?: number
  itemFileSize?: number
  canResume?: boolean
  minimumBytes?: number
  cookieParityOk?: boolean
}

function headerValue(headers: Record<string, string>, name: string): string {
  const direct = headers[name]
  if (typeof direct === 'string') return direct.trim()
  const target = name.toLowerCase()
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() === target && typeof value === 'string') return value.trim()
  }
  return ''
}

function knownPayloadBytes(input: AcceleratedTakeoverInput): number {
  if (input.responseHeaders) {
    const contentRange = headerValue(input.responseHeaders, 'content-range')
    const rangeTotal = Number(contentRange.match(/\/(\d+)$/)?.[1] || 0)
    if (Number.isFinite(rangeTotal) && rangeTotal > 0) return rangeTotal
    const contentLength = Number(headerValue(input.responseHeaders, 'content-length') || 0)
    if (Number.isFinite(contentLength) && contentLength > 0) return contentLength
  }
  const totalBytes = Number(input.itemTotalBytes || 0)
  if (Number.isFinite(totalBytes) && totalBytes > 0) return totalBytes
  const fileSize = Number(input.itemFileSize || 0)
  if (Number.isFinite(fileSize) && fileSize > 0) return fileSize
  return 0
}

const SINGLE_USE_AUTH_QUERY_KEYS = new Set([
  'samlresponse',
  'samlrequest',
  'samlart',
  'oauth_verifier',
  'one_time_token',
  'onetime_token',
  'single_use_token',
  'one-time-token',
  'single-use',
])

const PRESIGNED_OBJECT_STORAGE_KEYS = new Set([
  'x-amz-signature',
  'x-amz-credential',
  'x-amz-algorithm',
  'x-goog-signature',
  'x-goog-credential',
])

const AUTH_CALLBACK_PATH = /(?:^|\/)(?:sso|saml2?|oauth2?|cas|oidc|auth\/callback|login\/callback)(?:\/|$)/i

/**
 * True only for genuine one-time OAuth/SAML/SSO callback or ticket redemption
 * URLs that cannot be replayed by Core.
 *
 * Time-windowed object storage / CDN presigned URLs (AWS S3 / GitHub / R2
 * `X-Amz-Signature`, GCS `X-Goog-Signature`, Azure SAS `sig`+`se`, CDN tokens)
 * remain valid for minutes/hours and MUST return `false` so Core accelerates them.
 */
export function looksSingleUseAuthorizedDownload(value?: string): boolean {
  if (!value) return false
  if (isAuthenticationNavigation(value)) return true
  try {
    const url = new URL(value)
    const keys = new Set<string>()
    url.searchParams.forEach((_val, key) => keys.add(key.toLowerCase()))
    for (const presignedKey of PRESIGNED_OBJECT_STORAGE_KEYS) {
      if (keys.has(presignedKey)) return false
    }
    if (keys.has('sig') && (keys.has('se') || keys.has('sv') || keys.has('sp'))) return false
    for (const singleUseKey of SINGLE_USE_AUTH_QUERY_KEYS) {
      if (keys.has(singleUseKey)) return true
    }
    if (keys.has('oauth_token') && (keys.has('oauth_verifier') || AUTH_CALLBACK_PATH.test(url.pathname))) {
      return true
    }
    if (keys.has('code') && keys.has('state')) return true
    if (keys.has('ticket') && AUTH_CALLBACK_PATH.test(url.pathname)) return true
    return false
  } catch {
    return false
  }
}

/**
 * Decide whether an in-progress browser `file`/`media` download can be handed
 * off directly to Desktop Core (`op: 'offer'`) instead of waiting for the
 * browser's single-threaded download to finish (`offer_local_file`).
 *
 * Rust Core's `http_engine` probes `Range: bytes=0-0` itself—accelerating with
 * multi-connections when supported or streaming cleanly over a single connection
 * when `Accept-Ranges` or `Content-Length` is absent. Note Chrome always sets
 * `item.canResume === false` on newly created `downloads.onCreated` items.
 */
export function canAccelerateBrowserDownload(input: AcceleratedTakeoverInput): boolean {
  if (input.kind !== 'file' && input.kind !== 'media') return false
  if (input.itemState !== undefined && input.itemState !== 'in_progress') return false
  if (String(input.method || '').toUpperCase() !== 'GET') return false
  let normalizedUrl = ''
  try {
    const parsed = new URL(String(input.url || ''))
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false
    normalizedUrl = parsed.href
  } catch {
    return false
  }
  if (looksSingleUseAuthorizedDownload(normalizedUrl)) return false
  if (input.statusCode !== undefined && input.statusCode !== 200 && input.statusCode !== 206) return false

  const configuredMin = Number(input.minimumBytes)
  const threshold = Number.isFinite(configuredMin) && configuredMin > 0 ? configuredMin : 0
  const payloadBytes = knownPayloadBytes(input)
  if (threshold > 0 && payloadBytes > 0 && payloadBytes < threshold) return false

  if (input.responseHeaders) {
    const hasValidator = Boolean(
      headerValue(input.responseHeaders, 'etag')
      || headerValue(input.responseHeaders, 'last-modified'),
    )
    const cacheControl = headerValue(input.responseHeaders, 'cache-control')
    const noStore = /(?:^|,)\s*no-store\b/i.test(cacheControl)
    if (noStore && !hasValidator) return false
  }

  if (input.cookieParityOk === false) return false
  return true
}

export function isOrdinaryFileKind(kind?: ResourceKind | null): boolean {
  return kind === 'file' || kind === 'magnet'
}

/**
 * Autoplay and iframe preloads can look like a media navigation. Ordinary
 * zip/exe/pdf responses never autoplay, so a classified file does not need a
 * click before the desktop prompt.
 */
export function earlyTakeoverRequiresClick(kind?: ResourceKind | null, disposition = ''): boolean {
  // attachment 已明确要求保存；脚本导航、表单跳转和地址栏下载不一定存在点击记录。
  return !isOrdinaryFileKind(kind) && !/^\s*attachment\b/i.test(disposition)
}

/** Classified files already have a DownloadItem; do not stall the prompt. */
export function clickIntentPollsForKind(kind?: ResourceKind | null): number {
  return isOrdinaryFileKind(kind) ? 4 : 12
}

/**
 * A main-frame/sub-frame response that is already a file, not an HTML page
 * that happened to be typed as `kind: file` in a test double.
 */
export function ordinaryFileResponseIdentified(resource: {
  kind?: ResourceKind | null
  url?: string
  filename?: string
  mimeType?: string
}): boolean {
  if (!isOrdinaryFileKind(resource.kind)) return false
  const mime = String(resource.mimeType || '')
  return looksLikeDownloadFile(resource.url || '')
    || looksLikeDownloadFile(resource.filename || '')
    || isConcreteDownloadMime(mime)
    || /octet-stream/i.test(mime)
}
