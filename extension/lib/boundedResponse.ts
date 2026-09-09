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

  const decoder = new TextDecoder()
  let bytes = 0
  let text = ''
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) {
        text += decoder.decode()
        return text
      }
      bytes += value.byteLength
      if (bytes > limit) {
        await reader.cancel()
        return null
      }
      text += decoder.decode(value, { stream: true })
    }
  } finally {
    reader.releaseLock()
  }
}
