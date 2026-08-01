export interface CurlDownloadRequest {
  url: string
  method: string
  body: string
  referer: string
  origin: string
  userAgent: string
  cookie: string
  headers: Record<string, string>
}

function tokenize(command: string): string[] {
  const values: string[] = []
  let value = ''
  let quote = ''
  let escaped = false
  for (const char of command.replace(/(?:\\|\^|`)\r?\n/g, ' ')) {
    if (escaped) { value += char; escaped = false; continue }
    if (char === '\\' && quote !== "'") { escaped = true; continue }
    if (quote) {
      if (char === quote) quote = ''
      else value += char
      continue
    }
    if (char === '"' || char === "'") { quote = char; continue }
    if (/\s/.test(char)) {
      if (value) { values.push(value); value = '' }
      continue
    }
    value += char
  }
  if (escaped) value += '\\'
  if (quote) throw new Error('cURL 命令的引号没有闭合')
  if (value) values.push(value)
  return values
}

function basicAuth(value: string): string {
  const bytes = new TextEncoder().encode(value)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return `Basic ${btoa(binary)}`
}

export function parseCurlCommand(command: string): CurlDownloadRequest | null {
  const args = tokenize(command.trim())
  if (!/^curl(?:\.exe)?$/i.test(args[0] || '')) return null
  let url = ''
  let method = 'GET'
  let body = ''
  const headers: Record<string, string> = {}
  const takeValue = (index: number, option: string) => {
    if (index + 1 >= args.length) throw new Error(`${option} 缺少参数`)
    return args[index + 1]
  }
  for (let index = 1; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === '--url') { url = takeValue(index, arg); index += 1; continue }
    if (arg === '-X' || arg === '--request') { method = takeValue(index, arg).toUpperCase(); index += 1; continue }
    if (arg === '-H' || arg === '--header') {
      const raw = takeValue(index, arg); index += 1
      const split = raw.indexOf(':')
      if (split > 0) headers[raw.slice(0, split).trim().toLowerCase()] = raw.slice(split + 1).trim()
      continue
    }
    if (arg === '-A' || arg === '--user-agent') { headers['user-agent'] = takeValue(index, arg); index += 1; continue }
    if (arg === '-e' || arg === '--referer') { headers.referer = takeValue(index, arg); index += 1; continue }
    if (arg === '-b' || arg === '--cookie') { headers.cookie = takeValue(index, arg); index += 1; continue }
    if (arg === '-u' || arg === '--user') { headers.authorization = basicAuth(takeValue(index, arg)); index += 1; continue }
    if (arg === '-d' || arg === '--data' || arg === '--data-raw' || arg === '--data-binary' || arg === '--data-urlencode') {
      body = takeValue(index, arg); index += 1
      if (body.startsWith('@')) throw new Error('不能导入引用本机文件的 cURL 请求体')
      if (method === 'GET') method = 'POST'
      continue
    }
    if (['-o', '--output', '--proxy', '--connect-timeout', '--max-time', '--retry', '--resolve', '--cacert', '--cert', '--key'].includes(arg)) {
      takeValue(index, arg); index += 1; continue
    }
    if (arg.startsWith('-')) {
      // Common switches without a value; unsupported transport flags can be
      // ignored because the desktop engine supplies its own safe defaults.
      continue
    }
    if (!url) url = arg
  }
  if (!/^https?:\/\//i.test(url)) throw new Error('cURL 命令中没有有效的 HTTP(S) 地址')
  if (body && !headers['content-type']) headers['content-type'] = 'application/x-www-form-urlencoded'
  const referer = headers.referer || ''
  const origin = headers.origin || ''
  const userAgent = headers['user-agent'] || ''
  const cookie = headers.cookie || ''
  for (const name of ['referer', 'origin', 'user-agent', 'cookie', 'content-length', 'range']) delete headers[name]
  return { url, method, body, referer, origin, userAgent, cookie, headers }
}
