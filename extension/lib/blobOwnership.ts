/**
 * Recover the HTTP URL that produced Blob/File parts. Pages often wrap a
 * fetched ArrayBuffer in `new Blob([bytes])` before createObjectURL; the
 * object URL itself has no origin unless that wrapping is correlated.
 *
 * Ownership is inherited only when exactly one non-empty part is proven to
 * come from an HTTP response. Multiple source-backed parts are still a new
 * client-side payload even when they came from the same URL: concatenating a
 * response twice must not be replayed as one fetch of that URL. Generated or
 * transformed content likewise stays browser-owned.
 */
export function inheritHttpBufferSource(
  parts: unknown[] | undefined,
  lookup: (value: object) => string | undefined,
): string {
  let inherited = ''
  for (const part of parts || []) {
    if (typeof part === 'string') {
      if (part.length) return ''
      continue
    }
    if (!part || (typeof part !== 'object' && typeof part !== 'function')) return ''

    const source = lookup(part as object)
      || (ArrayBuffer.isView(part) ? lookup(part.buffer) : undefined)
      || ''
    if (/^https?:\/\//i.test(source)) {
      if (inherited) return ''
      inherited = source
      continue
    }

    // Empty parts do not change the wrapped payload and can be ignored even
    // when they have no ownership record. Any other untracked part means the
    // page generated or transformed content, so correlation is unsafe.
    if (part instanceof ArrayBuffer && part.byteLength === 0) continue
    if (ArrayBuffer.isView(part) && part.byteLength === 0) continue
    if (typeof Blob !== 'undefined' && part instanceof Blob && part.size === 0) continue
    return ''
  }
  return inherited
}

function derivedObjectSize(value: object): number | undefined {
  try {
    const candidate = value as { size?: unknown, byteLength?: unknown }
    if (candidate.size !== undefined) {
      const size = Number(candidate.size)
      if (Number.isFinite(size) && size >= 0) return size
    }
    if (candidate.byteLength !== undefined) {
      const bytes = Number(candidate.byteLength)
      if (Number.isFinite(bytes) && bytes >= 0) return bytes
    }
  } catch {}
  return undefined
}

/** Copy HTTP ownership only across content-preserving Blob.slice()-style derivations. */
export function copyHttpBufferSource(
  sourceObject: object,
  target: object,
  lookup: (value: object) => string | undefined,
  remember: (value: object, sourceUrl: string) => void,
): void {
  const sourceSize = derivedObjectSize(sourceObject)
  const targetSize = derivedObjectSize(target)
  if (sourceSize !== undefined && targetSize !== undefined && sourceSize !== targetSize) return
  const source = lookup(sourceObject) || ''
  if (/^https?:\/\//i.test(source)) remember(target, source)
}
