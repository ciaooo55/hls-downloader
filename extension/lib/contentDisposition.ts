/**
 * Parse the safe filename portion of Content-Disposition.  Download servers
 * use both RFC 5987 (filename*) and legacy filename parameters; treating only
 * one of them causes garbled CJK names and truncates quoted names containing a
 * semicolon.  The result is display-safe but filename sanitization remains the
 * desktop application's responsibility.
 */
function parseParameters(value: string): Map<string, string> {
  const params = new Map<string, string>()
  let index = 0
  while (index < value.length) {
    while (index < value.length && (value[index] === ';' || /\s/.test(value[index]))) index += 1
    const keyStart = index
    while (index < value.length && value[index] !== '=' && value[index] !== ';') index += 1
    const key = value.slice(keyStart, index).trim().toLowerCase()
    if (index >= value.length || value[index] === ';') {
      index += 1
      continue
    }
    index += 1 // skip '='
    while (index < value.length && /\s/.test(value[index])) index += 1
    let parsed = ''
    if (value[index] === '"') {
      index += 1
      while (index < value.length && value[index] !== '"') {
        if (value[index] === '\\' && index + 1 < value.length) {
          parsed += value[index + 1]
          index += 2
        } else {
          parsed += value[index]
          index += 1
        }
      }
      if (value[index] === '"') index += 1
      while (index < value.length && value[index] !== ';') index += 1
    } else {
      const valStart = index
      while (index < value.length && value[index] !== ';') index += 1
      parsed = value.slice(valStart, index).trim()
    }
    if (key && !params.has(key)) params.set(key, parsed.trim())
  }
  return params
}

function clean(value: string): string {
  const cleaned = value.replace(/[\u0000-\u001f\u007f]/g, '').trim()
  let bounded = cleaned.slice(0, 512)
  const last = bounded.charCodeAt(bounded.length - 1)
  if (last >= 0xd800 && last <= 0xdbff) bounded = bounded.slice(0, -1)
  return bounded
}

function percentBytes(value: string, charset: string): Uint8Array {
  const bytes: number[] = []
  const latin1 = charset === 'iso-8859-1' || charset === 'latin1'
  const encoder = new TextEncoder()
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === '%' && /^[0-9a-f]{2}$/i.test(value.slice(index + 1, index + 3))) {
      bytes.push(Number.parseInt(value.slice(index + 1, index + 3), 16))
      index += 2
    } else {
      const code = value.charCodeAt(index)
      if (code <= 0x7f) {
        bytes.push(code)
      } else if (latin1 && code <= 0xff) {
        bytes.push(code)
      } else if (code >= 0xd800 && code <= 0xdbff && index + 1 < value.length) {
        const low = value.charCodeAt(index + 1)
        if (low >= 0xdc00 && low <= 0xdfff) {
          bytes.push(...encoder.encode(value.slice(index, index + 2)))
          index += 1
        } else {
          bytes.push(...encoder.encode(value[index]))
        }
      } else {
        bytes.push(...encoder.encode(value[index]))
      }
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
    return new TextDecoder(charset, { fatal: true }).decode(percentBytes(match[2], charset))
  } catch {
    return ''
  }
}

export function contentDispositionFilename(value = ''): string {
  const params = parseParameters(value)
  const extended = params.get('filename*') || ''
  if (extended) {
    const decoded = clean(decodeExtended(extended))
    if (decoded) return decoded
  }
  return clean(params.get('filename') || '')
}
