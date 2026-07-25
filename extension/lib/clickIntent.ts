const DOWNLOAD_HINT = /(?:^|[\s_:/-])(?:download|save|export|install|offline)(?:$|[\s_:/-])|下载|保存|另存|导出|安装|离线|缓存/i

export function isLikelyDownloadControl(hints: Array<string | null | undefined>): boolean {
  const value = hints.filter(Boolean).join(' ').replace(/([a-z])([A-Z])/g, '$1 $2')
  return DOWNLOAD_HINT.test(` ${value} `)
}

/**
 * A page may expose a download as a normal link, a JavaScript link, or a
 * button.  Record only concrete destinations, explicit download wording, or
 * a user-forced Ctrl click; this gives generated downloads an intent trail
 * without treating ordinary navigation as a download request.
 */
export function shouldTrackDownloadIntent(input: {
  directHref?: string
  hintedHref?: string
  ctrlForce?: boolean
  hints?: Array<string | null | undefined>
}): boolean {
  return Boolean(
    input.directHref
    || input.hintedHref
    || input.ctrlForce
    || isLikelyDownloadControl(input.hints || []),
  )
}
