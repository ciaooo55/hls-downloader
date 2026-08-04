import { describe, expect, it } from 'vitest'
import { readBoundedResponseText } from './boundedResponse'

describe('bounded response reader', () => {
  it('reads a normal response and honors the byte limit', async () => {
    expect(await readBoundedResponseText(new Response('héllo'), 16)).toBe('héllo')
    expect(await readBoundedResponseText(new Response('0123456789'), 4)).toBeNull()
  })

  it('stops a chunked response as soon as it exceeds the limit', async () => {
    let canceled = false
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('1234'))
        controller.enqueue(new TextEncoder().encode('5678'))
      },
      cancel() { canceled = true },
    })
    const response = new Response(stream)
    expect(await readBoundedResponseText(response, 6)).toBeNull()
    expect(canceled).toBe(true)
  })
})
