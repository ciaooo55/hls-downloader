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

describe('native resource string safety', () => {
  it('keeps scalar-safe strings unchanged and sanitizes invalid or truncated surrogates', async () => {
    const port = new FakePort()
    const bridge = new NativeBridge(() => port)

    const normalTitle = '普通页面标题 😀'
    const normalFilename = '视频 😀.mp4'
    const normal = bridge.request({ op: 'download', resource: { title: normalTitle, filename: normalFilename } })
    expect((port.posted[0].resource as Record<string, unknown>).title).toBe(normalTitle)
    expect((port.posted[0].resource as Record<string, unknown>).filename).toBe(normalFilename)
    port.onMessage.emit({ ok: true, __request_id: port.posted[0].__request_id })
    await normal

    const malformedTitle = `before\uD800middle\uDC00after`
    const malformedFilename = `file\uD800name\uDC00.mp4`
    const malformed = bridge.request({
      op: 'download',
      resource: { title: malformedTitle, filename: malformedFilename },
    })
    const malformedResource = port.posted[1].resource as Record<string, unknown>
    const sanitizedTitle = String(malformedResource.title)
    const sanitizedFilename = String(malformedResource.filename)
    expect(sanitizedTitle).toBe('before�middle�after')
    expect(sanitizedFilename).toBe('file�name�.mp4')
    expect(`${sanitizedTitle}${sanitizedFilename}`).not.toMatch(/[\uD800-\uDFFF]/)
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

  it('sanitizes a posted copy without mutating caller resource metadata', async () => {
    const port = new FakePort()
    const bridge = new NativeBridge(() => port)
    const title = `page\uD800title`
    const filename = `video\uDC00.mp4`
    const resource = { title, filename }

    const request = bridge.request({ op: 'download', resource })
    const postedResource = port.posted[0].resource as Record<string, unknown>

    expect(postedResource).not.toBe(resource)
    expect(postedResource.title).toBe('page�title')
    expect(postedResource.filename).toBe('video�.mp4')
    expect(resource).toEqual({ title, filename })

    port.onMessage.emit({ ok: true, __request_id: port.posted[0].__request_id })
    await request
    bridge.close()
  })
})
