/**
 * Parse the safe filename portion of Content-Disposition.  Download servers
 * use RFC 5987 (filename*), RFC 2047 encoded-words (=?UTF-8?B?...?= / =?UTF-8?Q?...?=),
 * raw UTF-8 bytes in legacy filename="...", and quoted semicolons.  The result is
 * display-safe while final filesystem sanitization remains Core's responsibility.
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

const SUPPORTED_CHARSETS = new Set(['utf-8', 'utf8', 'iso-8859-1', 'latin1', 'us-ascii'])

function normalizeDecoderCharset(charset: string): string | null {
  const lower = charset.trim().toLowerCase()
  if (!SUPPORTED_CHARSETS.has(lower)) return null
  if (lower === 'utf8') return 'utf-8'
  if (lower === 'latin1') return 'iso-8859-1'
  return lower
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
  const charset = normalizeDecoderCharset(match[1] || 'utf-8')
  if (!charset) return ''
  try {
    return new TextDecoder(charset, { fatal: true }).decode(percentBytes(match[2], charset))
  } catch {
    return ''
  }
}

function decodeBase64Bytes(payload: string): Uint8Array | null {
  const normalized = payload.replace(/\s+/g, '')
  if (!normalized || !/^[A-Za-z0-9+/]*={0,2}$/.test(normalized) || normalized.length % 4 === 1) return null
  try {
    const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=')
    const binary = atob(padded)
    const bytes = new Uint8Array(binary.length)
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i)
    return bytes
  } catch {
    return null
  }
}

function decodeQuotedPrintableBytes(payload: string): Uint8Array {
  const bytes: number[] = []
  for (let i = 0; i < payload.length; i += 1) {
    const ch = payload[i]
    if (ch === '_') {
      bytes.push(0x20)
    } else if (ch === '=' && /^[0-9a-f]{2}$/i.test(payload.slice(i + 1, i + 3))) {
      bytes.push(Number.parseInt(payload.slice(i + 1, i + 3), 16))
      i += 2
    } else {
      bytes.push(payload.charCodeAt(i) & 0xff)
    }
  }
  return new Uint8Array(bytes)
}

const RFC2047_WORD = /=\?([^?\s]+)\?([bBqQ])\?([^?]*)\?=/g

function decodeRfc2047(value: string): string {
  if (!value.includes('=?')) return value
  type Token =
    | { kind: 'text'; text: string }
    | { kind: 'word'; raw: string; charset: string; bytes: Uint8Array }

  const tokens: Token[] = []
  let lastIndex = 0
  RFC2047_WORD.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = RFC2047_WORD.exec(value)) !== null) {
    if (match.index > lastIndex) {
      tokens.push({ kind: 'text', text: value.slice(lastIndex, match.index) })
    }
    const charset = normalizeDecoderCharset(match[1])
    const mode = match[2].toUpperCase()
    const bytes = charset
      ? (mode === 'B' ? decodeBase64Bytes(match[3]) : decodeQuotedPrintableBytes(match[3]))
      : null
    if (charset && bytes) {
      tokens.push({ kind: 'word', raw: match[0], charset, bytes })
    } else {
      tokens.push({ kind: 'text', text: match[0] })
    }
    lastIndex = match.index + match[0].length
  }
  if (!tokens.length) return value
  if (lastIndex < value.length) tokens.push({ kind: 'text', text: value.slice(lastIndex) })

  // RFC 2047 §6.2: ignore linear whitespace between adjacent encoded-words and
  // join adjacent same-charset byte runs so split multi-byte UTF-8 sequences decode cleanly.
  const compacted: Token[] = []
  for (let i = 0; i < tokens.length; i += 1) {
    const current = tokens[i]
    if (
      current.kind === 'text'
      && /^\s+$/.test(current.text)
      && compacted.at(-1)?.kind === 'word'
      && tokens[i + 1]?.kind === 'word'
    ) {
      continue
    }
    const prev = compacted.at(-1)
    if (current.kind === 'word' && prev?.kind === 'word' && prev.charset === current.charset) {
      const merged = new Uint8Array(prev.bytes.length + current.bytes.length)
      merged.set(prev.bytes, 0)
      merged.set(current.bytes, prev.bytes.length)
      compacted[compacted.length - 1] = {
        kind: 'word',
        raw: prev.raw + current.raw,
        charset: prev.charset,
        bytes: merged,
      }
      continue
    }
    compacted.push(current)
  }

  return compacted.map(token => {
    if (token.kind === 'text') return token.text
    try {
      return new TextDecoder(token.charset, { fatal: true }).decode(token.bytes)
    } catch {
      return token.raw
    }
  }).join('')
}

function decodeRawUtf8BytesIfMojibake(value: string): string {
  if (!value) return value
  let hasHighLatin1 = false
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i)
    // Already decoded to Unicode code points beyond Latin-1.
    if (code > 0xff) return value
    if (code >= 0x80) hasHighLatin1 = true
  }
  if (!hasHighLatin1) return value
  const bytes = new Uint8Array(value.length)
  for (let i = 0; i < value.length; i += 1) bytes[i] = value.charCodeAt(i)
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  } catch {
    // Genuine single-byte ISO-8859-1 (e.g. "café.pdf") stays intact.
    return value
  }
}

function decodeLegacyFilename(value: string): string {
  if (!value) return ''
  const unescaped = decodeRawUtf8BytesIfMojibake(value)
  return decodeRfc2047(unescaped)
}

export function contentDispositionFilename(value = ''): string {
  const params = parseParameters(value)
  const extended = params.get('filename*') || ''
  if (extended) {
    const decoded = clean(decodeExtended(extended))
    if (decoded) return decoded
  }
  return clean(decodeLegacyFilename(params.get('filename') || ''))
}
