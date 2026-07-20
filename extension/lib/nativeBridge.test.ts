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

describe('persistent native bridge', () => {
  it('reuses one native port and serializes requests', async () => {
    const port = new FakePort()
    const connect = vi.fn(() => port)
    const bridge = new NativeBridge(connect)
    const first = bridge.request({ op: 'ping' })
    const second = bridge.request({ op: 'offer' })
    expect(port.posted).toEqual([{ op: 'ping' }])
    port.onMessage.emit({ ok: true, value: 1 })
    await expect(first).resolves.toMatchObject({ value: 1 })
    expect(port.posted).toEqual([{ op: 'ping' }, { op: 'offer' }])
    port.onMessage.emit({ ok: true, value: 2 })
    await expect(second).resolves.toMatchObject({ value: 2 })
    expect(connect).toHaveBeenCalledTimes(1)
    bridge.close()
  })

  it('rejects the active request and reconnects after host disconnect', async () => {
    const firstPort = new FakePort()
    const secondPort = new FakePort()
    const connect = vi.fn()
      .mockReturnValueOnce(firstPort)
      .mockReturnValueOnce(secondPort)
    const disconnected = vi.fn()
    const bridge = new NativeBridge(connect, 30_000, disconnected)
    const first = bridge.request({ op: 'offer' })
    firstPort.onDisconnect.emit()
    await expect(first).rejects.toThrow('disconnected')
    expect(disconnected).toHaveBeenCalledOnce()
    const second = bridge.request({ op: 'ping' })
    secondPort.onMessage.emit({ ok: true })
    await expect(second).resolves.toMatchObject({ ok: true })
    expect(connect).toHaveBeenCalledTimes(2)
    bridge.close()
  })

  it('supports a longer timeout for a desktop confirmation request', async () => {
    vi.useFakeTimers()
    const port = new FakePort()
    const bridge = new NativeBridge(() => port, 100)
    const request = bridge.request({ op: 'wait_handoff' }, 1_000)
    await vi.advanceTimersByTimeAsync(200)
    expect(port.disconnect).not.toHaveBeenCalled()
    port.onMessage.emit({ ok: true, handoff: { status: 'accepted' } })
    await expect(request).resolves.toMatchObject({ ok: true })
    bridge.close()
    vi.useRealTimers()
  })
})
