/**
 * Update checks are network-bound. Keep any legacy/native error responses out
 * of the UI: older desktop cores may still send urllib/OpenSSL text.
 */
export function friendlyUpdateError(reason: unknown, fallback = '暂时无法完成更新操作，请稍后重试。'): string {
  const message = reason instanceof Error ? reason.message : ''
  const normalized = message.toLowerCase()
  if (normalized.includes('rate limit') || normalized.includes('github_rate_limited')) {
    return 'GitHub 暂时限制了匿名更新检查，请稍后重试。'
  }
  if (normalized.includes('ssl') || normalized.includes('urlopen') || normalized.includes('tls') || normalized.includes('eof') || normalized.includes('http error')) {
    return '网络连接不稳定，暂时无法检查更新。请稍后重试，或到 Release 页面手动下载。'
  }
  // Backend update errors are deliberately short Chinese sentences. Do not
  // surface a long implementation message from an older installed core.
  if (message && message.length <= 120 && !/[<>]/.test(message)) return message
  return fallback
}
