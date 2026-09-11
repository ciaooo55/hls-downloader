/** Read a response without allowing an untrusted manifest to grow unbounded. */
export async function readBoundedResponseText(
  response: Response,
  maxBytes: number,
): Promise<string | null> {
  const limit = Math.max(0, Math.floor(maxBytes))
  const declared = Number(response.headers.get('content-length') || 0)
  if (declared > limit) return null

  const reader = response.body?.getReader()
  if (!reader) {
    const text = await response.text()
    return new TextEncoder().encode(text).byteLength <= limit ? text : null
  }

  const decoder = new TextDecoder('utf-8', { fatal: true })
  let bytes = 0
  let text = ''
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) {
        try {
          text += decoder.decode()
        } catch {
          return null
        }
        return text
      }
      bytes += value.byteLength
      if (bytes > limit) {
        try { await reader.cancel() } catch {}
        return null
      }
      try {
        text += decoder.decode(value, { stream: true })
      } catch {
        try { await reader.cancel() } catch {}
        return null
      }
    }
  } finally {
    reader.releaseLock()
  }
}
