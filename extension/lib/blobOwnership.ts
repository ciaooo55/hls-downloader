/**
 * Recover the HTTP URL that produced Blob/File parts. Pages often wrap a
 * fetched ArrayBuffer in `new Blob([bytes])` before createObjectURL; the
 * object URL itself has no origin unless that wrapping is correlated.
 */
export function inheritHttpBufferSource(
  parts: unknown[] | undefined,
  lookup: (value: object) => string | undefined,
): string {
  for (const part of parts || []) {
    if (!part || (typeof part !== 'object' && typeof part !== 'function')) continue
    const source = lookup(part as object)
      || (ArrayBuffer.isView(part) ? lookup(part.buffer) : undefined)
      || ''
    if (/^https?:\/\//i.test(source)) return source
  }
  return ''
}

/** Copy HTTP ownership across Blob.slice() and similar derived objects. */
export function copyHttpBufferSource(
  sourceObject: object,
  target: object,
  lookup: (value: object) => string | undefined,
  remember: (value: object, sourceUrl: string) => void,
): void {
  const source = lookup(sourceObject) || ''
  if (/^https?:\/\//i.test(source)) remember(target, source)
}
