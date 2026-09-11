import { describe, expect, it, vi } from 'vitest'
import { NativeBridge, type NativePortLike } from './nativeBridge'

class FakeEvent<T extends (...args: any[]) => void> {
  listener: T | null = null
  addListener(listener: T) { this.listener = listener }
  emit(...args: Parameters<T>) { this.listener?.(...args) }
}

class FakePort implements NativePortLike {
  readonly posted: Record<string, unknown>[] = []
  readonly onMessage = new FakeEvent<(message: unknown) => void>()
  readonly onDisconnect = new FakeEvent<() => void>()
  disconnect = vi.fn(() => this.onDisconnect.emit())
  postMessage(message: Record<string, unknown>) { this.posted.push(message) }
}

describe('native resource title bounds', () => {
  it('keeps scalar-safe titles unchanged and sanitizes invalid or truncated surrogates', async () => {
    const port = new FakePort()
    const bridge = new NativeBridge(() => port)

    const normalTitle = '普通页面标题 😀'
    const normal = bridge.request({ op: 'download', resource: { title: normalTitle } })
    expect((port.posted[0].resource as Record<string, unknown>).title).toBe(normalTitle)
    port.onMessage.emit({ ok: true, __request_id: port.posted[0].__request_id })
    await normal

    const malformedTitle = `before\uD800middle\uDC00after`
    const malformed = bridge.request({ op: 'download', resource: { title: malformedTitle } })
    const sanitizedTitle = String((port.posted[1].resource as Record<string, unknown>).title)
    expect(sanitizedTitle).toBe('before�middle�after')
    expect(sanitizedTitle).not.toMatch(/[\uD800-\uDFFF]/)
    port.onMessage.emit({ ok: true, __request_id: port.posted[1].__request_id })
    await malformed

    const oversizedTitle = `${'a'.repeat(4095)}😀`
    const bounded = bridge.request({ op: 'download', resource: { title: oversizedTitle } })
    const postedTitle = String((port.posted[2].resource as Record<string, unknown>).title)
    expect(postedTitle).toBe('a'.repeat(4095))
    expect(postedTitle).not.toMatch(/[\uD800-\uDBFF]$/)
    port.onMessage.emit({ ok: true, __request_id: port.posted[2].__request_id })
    await bounded

    bridge.close()
  })
})
