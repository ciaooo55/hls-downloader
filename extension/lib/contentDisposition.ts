/**
 * Parse the safe filename portion of Content-Disposition.  Download servers
 * use both RFC 5987 (filename*) and legacy filename parameters; treating only
 * one of them causes garbled CJK names and truncates quoted names containing a
 * semicolon.  The result is display-safe but filename sanitization remains the
 * desktop application's responsibility.
 */
function parameter(value: string, name: string): string {
  const expression = new RegExp(`(?:^|;)\\s*${name}\\s*=\\s*(?:"((?:\\\\.|[^"])*)"|([^;]*))`, 'i')
  const match = value.match(expression)
  if (!match) return ''
  const raw = (match[1] ?? match[2] ?? '').trim()
  return match[1] === undefined ? raw : raw.replace(/\\(.)/g, '$1')
}

function clean(value: string): string {
  const cleaned = value.replace(/[\u0000-\u001f\u007f]/g, '').trim()
  let bounded = cleaned.slice(0, 512)
  const last = bounded.charCodeAt(bounded.length - 1)
  if (last >= 0xd800 && last <= 0xdbff) bounded = bounded.slice(0, -1)
  return bounded
}

function percentBytes(value: string): Uint8Array {
  const bytes: number[] = []
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === '%' && /^[0-9a-f]{2}$/i.test(value.slice(index + 1, index + 3))) {
      bytes.push(Number.parseInt(value.slice(index + 1, index + 3), 16))
      index += 2
    } else {
      const code = value.charCodeAt(index)
      if (code <= 0x7f) bytes.push(code)
      else bytes.push(...new TextEncoder().encode(value[index]))
    }
  }
  return new Uint8Array(bytes)
}

function decodeExtended(value: string): string {
  const match = value.match(/^([^']*)'[^']*'(.*)$/)
  if (!match) {
    try { return decodeURIComponent(value) } catch { return value }
  }
  const charset = (match[1] || 'utf-8').trim().toLowerCase()
  if (!['utf-8', 'utf8', 'iso-8859-1', 'latin1', 'us-ascii'].includes(charset)) return ''
  try {
    return new TextDecoder(charset, { fatal: true }).decode(percentBytes(match[2]))
  } catch {
    return ''
  }
}

export function contentDispositionFilename(value = ''): string {
  const extended = parameter(value, 'filename\\*')
  if (extended) {
    const decoded = clean(decodeExtended(extended))
    if (decoded) return decoded
  }
  return clean(parameter(value, 'filename'))
}
