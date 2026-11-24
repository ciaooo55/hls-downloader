import { isConcreteDownloadMime, looksLikeDownloadFile, type ResourceKind } from './resources'

/** Minimum known payload size (1 MiB) before handing off a browser download for Core multi-connection acceleration. */
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

/**
 * Decide whether an in-progress browser `file`/`media` download can safely be
 * handed off to Desktop Core for multi-connection HTTP acceleration instead of
 * staying in the browser's single-threaded transfer and importing locally.
 *
 * Fails closed to browser preservation whenever range resumability, size,
 * repeatability, HTTP(S) GET semantics, or cookie parity cannot be proven.
 */
export function canAccelerateBrowserDownload(input: AcceleratedTakeoverInput): boolean {
  if (input.kind !== 'file' && input.kind !== 'media') return false
  if (input.itemState !== undefined && input.itemState !== 'in_progress') return false
  if (String(input.method || '').toUpperCase() !== 'GET') return false
  try {
    const parsed = new URL(String(input.url || ''))
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false
  } catch {
    return false
  }
  if (!input.responseHeaders) return false
  if (input.statusCode !== undefined && input.statusCode !== 200 && input.statusCode !== 206) return false

  const acceptRanges = headerValue(input.responseHeaders, 'accept-ranges')
  const supportsByteRanges = acceptRanges
    .split(',')
    .some(token => token.trim().toLowerCase() === 'bytes')
  if (!supportsByteRanges) return false

  const configuredMin = Number(input.minimumBytes)
  const threshold = Math.max(
    ACCELERATED_TAKEOVER_MIN_BYTES,
    Number.isFinite(configuredMin) && configuredMin > 0 ? configuredMin : 0,
  )
  if (knownPayloadBytes(input) < threshold) return false

  const hasValidator = Boolean(
    headerValue(input.responseHeaders, 'etag')
    || headerValue(input.responseHeaders, 'last-modified'),
  )
  const cacheControl = headerValue(input.responseHeaders, 'cache-control')
  const noStore = /(?:^|,)\s*no-store\b/i.test(cacheControl)
  if (noStore && !hasValidator) return false
  if (!hasValidator && input.canResume === false) return false

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
