const DOWNLOAD_HINT = /(?:^|[\s_:/-])(?:download|save|export|install|offline)(?:$|[\s_:/-])|下载|保存|另存|导出|安装|离线|缓存/i

export function isLikelyDownloadControl(hints: Array<string | null | undefined>): boolean {
  const value = hints.filter(Boolean).join(' ').replace(/([a-z])([A-Z])/g, '$1 $2')
  return DOWNLOAD_HINT.test(` ${value} `)
}
