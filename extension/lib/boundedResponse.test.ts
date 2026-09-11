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

  it('fails closed when canceling an oversized response rejects', async () => {
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('oversized'))
      },
      cancel() { return Promise.reject(new Error('cancel failed')) },
    })

    await expect(readBoundedResponseText(new Response(stream), 4)).resolves.toBeNull()
  })

  it('preserves split UTF-8 and rejects malformed byte sequences', async () => {
    const encoded = new TextEncoder().encode('é')
    const valid = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoded.slice(0, 1))
        controller.enqueue(encoded.slice(1))
        controller.close()
      },
    })
    expect(await readBoundedResponseText(new Response(valid), 4)).toBe('é')

    let canceled = false
    const malformed = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(Uint8Array.of(0x66, 0x6f, 0x6f, 0xff))
      },
      cancel() { canceled = true },
    })
    expect(await readBoundedResponseText(new Response(malformed), 8)).toBeNull()
    expect(canceled).toBe(true)

    const truncated = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(Uint8Array.of(0xc3))
        controller.close()
      },
    })
    expect(await readBoundedResponseText(new Response(truncated), 8)).toBeNull()
  })
})
