export function normalizeTvboxEndpoint(raw: string): string {
  const value = String(raw || '').trim().replace(/\/+$/, '')
  if (!value) throw new Error('请先在插件面板设置电视推送地址')
  let parsed: URL
  try { parsed = new URL(value) } catch { throw new Error('电视推送地址格式不正确') }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error('电视推送地址需以 http:// 或 https:// 开头')
  }
  parsed.hash = ''
  parsed.pathname = parsed.pathname.replace(/\/action\/?$/i, '') || '/'
  return parsed.href.replace(/\/$/, '')
}

export function tvboxActionUrl(endpoint: string): string {
  return `${normalizeTvboxEndpoint(endpoint)}/action`
}

export function tvboxPushBody(url: string): string {
  return new URLSearchParams({ do: 'push', url: String(url || '') }).toString()
}

export function tvboxPushGetUrl(endpoint: string, url: string): string {
  return `${tvboxActionUrl(endpoint)}?${tvboxPushBody(url)}`
}

export function tvboxResponseError(text: string): string {
  const value = String(text || '').trim()
  if (!value) return ''
  try {
    const body = JSON.parse(value) as Record<string, unknown>
    if (body.ok === false || body.success === false || body.error) {
      return String(body.error || body.message || '电视拒绝了推送')
    }
    if (typeof body.msg === 'string' && /失败|错误|拒绝|fail|error/i.test(body.msg)) return body.msg
  } catch {
    if (/^(?:error|fail|failed)\b/i.test(value)) return value
  }
  return ''
}
