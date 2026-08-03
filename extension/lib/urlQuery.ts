const ACCESS_QUERY = /^(?:token|auth|authorization|signature|sig|expires?|expiry|policy|key-pair-id|hdnea|hmac|jwt|session|sessionid|access[_-]?key|x-amz-.+|pkey|psch|playlisttype|validfrom|validto|ipa|hdl|hash|s|e|_t)$/i

function decodedName(pair: string): string {
  const raw = pair.split('=', 1)[0]
  try { return decodeURIComponent(raw.replace(/\+/g, ' ')) } catch { return raw }
}

function splitRaw(value: string): { base: string, query: string, fragment: string } {
  const hashAt = value.indexOf('#')
  const fragment = hashAt >= 0 ? value.slice(hashAt) : ''
  const withoutFragment = hashAt >= 0 ? value.slice(0, hashAt) : value
  const queryAt = withoutFragment.indexOf('?')
  return queryAt >= 0
    ? { base: withoutFragment.slice(0, queryAt), query: withoutFragment.slice(queryAt + 1), fragment }
    : { base: withoutFragment, query: '', fragment }
}

/** Remove selected query names without normalizing any remaining signature bytes. */
export function removeRawQueryParameters(value: string, names: Set<string>): string {
  const parts = splitRaw(value)
  if (!parts.query) return `${parts.base}${parts.fragment}`
  const kept = parts.query.split('&').filter(pair => pair && !names.has(decodedName(pair).toLowerCase()))
  return `${parts.base}${kept.length ? `?${kept.join('&')}` : ''}${parts.fragment}`
}

/** Carry known bearer/signature fields to an unsigned same-origin child URI. */
export function inheritManifestAccessQuery(baseUrl: string, resolvedUrl: string): string {
  try {
    const base = new URL(baseUrl)
    const child = new URL(resolvedUrl)
    if (!base.search || base.origin !== child.origin) return resolvedUrl
    const rawPairs = splitRaw(baseUrl).query.split('&').filter(Boolean)
    const childParts = splitRaw(resolvedUrl)
    const childPairs = childParts.query.split('&').filter(Boolean)
    const childNames = new Set(childPairs.map(pair => decodedName(pair).toLowerCase()))
    const names = new Set(rawPairs.map(pair => decodedName(pair).toLowerCase()))
    const terseSignature = names.has('s') && names.has('e')
    const inherited = rawPairs.filter(pair => {
      const name = decodedName(pair)
      const lowered = name.toLowerCase()
      if (['_hls_msn', '_hls_part', '_hls_skip'].includes(lowered)) return false
      if (childNames.has(lowered)) return false
      return ACCESS_QUERY.test(name) || (terseSignature && ['s', 'e', '_t'].includes(lowered))
    })
    if (!inherited.length) return resolvedUrl
    return `${childParts.base}?${[...childPairs, ...inherited].join('&')}${childParts.fragment}`
  } catch {
    return resolvedUrl
  }
}
