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
    expect(port.posted).toHaveLength(1)
    expect(port.posted[0]).toMatchObject({ op: 'ping' })
    port.onMessage.emit({ ok: true, value: 1 })
    await expect(first).resolves.toMatchObject({ value: 1 })
    expect(port.posted).toHaveLength(2)
    expect(port.posted[1]).toMatchObject({ op: 'offer' })
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

  it('retries an idempotent request once after a disconnect', async () => {
    const firstPort = new FakePort()
    const secondPort = new FakePort()
    const bridge = new NativeBridge(vi.fn()
      .mockReturnValueOnce(firstPort)
      .mockReturnValueOnce(secondPort))
    const request = bridge.request({ op: 'ping' }, 30_000, 1)
    const requestId = String(firstPort.posted[0].__request_id)
    firstPort.onDisconnect.emit()
    expect(secondPort.posted[0]).toMatchObject({ op: 'ping', __request_id: requestId })
    secondPort.onMessage.emit({ ok: true, __request_id: requestId })
    await expect(request).resolves.toMatchObject({ ok: true })
    bridge.close()
  })

  it('retries when opening the native port itself fails transiently', async () => {
    vi.useFakeTimers()
    const port = new FakePort()
    const connect = vi.fn()
      .mockImplementationOnce(() => { throw new Error('host is starting') })
      .mockReturnValueOnce(port)
    const bridge = new NativeBridge(connect)

    const request = bridge.request({ op: 'offer' }, 30_000, 1)
    await vi.advanceTimersByTimeAsync(81)
    expect(connect).toHaveBeenCalledTimes(2)
    expect(port.posted[0]).toMatchObject({ op: 'offer' })
    port.onMessage.emit({ ok: true, __request_id: port.posted[0].__request_id })
    await expect(request).resolves.toMatchObject({ ok: true })
    bridge.close()
    vi.useRealTimers()
  })

  it('ignores a stale response from a replaced native port', async () => {
    vi.useFakeTimers()
    const firstPort = new FakePort()
    const secondPort = new FakePort()
    const bridge = new NativeBridge(vi.fn()
      .mockReturnValueOnce(firstPort)
      .mockReturnValueOnce(secondPort), 100)
    const timedOut = bridge.request({ op: 'offer' })
    const timedOutAssertion = expect(timedOut).rejects.toThrow('timed out')
    await vi.advanceTimersByTimeAsync(101)
    await timedOutAssertion
    const active = bridge.request({ op: 'ping' })
    firstPort.onMessage.emit({ ok: true, value: 'stale' })
    secondPort.onMessage.emit({ ok: true, value: 'fresh' })
    await expect(active).resolves.toMatchObject({ value: 'fresh' })
    bridge.close()
    vi.useRealTimers()
  })
})
