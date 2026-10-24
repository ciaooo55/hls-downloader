import { isConcreteDownloadMime, looksLikeDownloadFile, type ResourceKind } from './resources'

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
