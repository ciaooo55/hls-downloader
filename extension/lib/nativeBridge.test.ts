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
    const first = bridge.request({ op: 'offer' })
    const second = bridge.request({ op: 'download' })
    expect(port.posted).toHaveLength(1)
    expect(port.posted[0]).toMatchObject({ op: 'offer' })
    port.onMessage.emit({ ok: true, value: 1, __request_id: port.posted[0].__request_id })
    await expect(first).resolves.toMatchObject({ value: 1 })
    expect(port.posted).toHaveLength(2)
    expect(port.posted[1]).toMatchObject({ op: 'download' })
    port.onMessage.emit({ ok: true, value: 2, __request_id: port.posted[1].__request_id })
    await expect(second).resolves.toMatchObject({ value: 2 })
    expect(connect).toHaveBeenCalledTimes(1)
    bridge.close()
  })

  it('preempts an active status request for an interactive offer', async () => {
    const firstPort = new FakePort()
    const secondPort = new FakePort()
    const bridge = new NativeBridge(vi.fn()
      .mockReturnValueOnce(firstPort)
      .mockReturnValueOnce(secondPort))
    const active = bridge.request({ op: 'handoff_status' })
    const heartbeat = bridge.request({ op: 'ping' })
    const offer = bridge.request({ op: 'offer' })

    expect(firstPort.disconnect).toHaveBeenCalledOnce()
    expect(secondPort.posted[0]).toMatchObject({ op: 'offer' })
    // A buffered response from the detached port must not complete the offer.
    firstPort.onMessage.emit({ ok: true, __request_id: firstPort.posted[0].__request_id })
    secondPort.onMessage.emit({ ok: true, __request_id: secondPort.posted[0].__request_id })
    await expect(offer).resolves.toMatchObject({ ok: true })

    expect(secondPort.posted[1]).toMatchObject({ op: 'handoff_status' })
    secondPort.onMessage.emit({ ok: true, __request_id: secondPort.posted[1].__request_id })
    await expect(active).resolves.toMatchObject({ ok: true })
    expect(secondPort.posted[2]).toMatchObject({ op: 'ping' })
    secondPort.onMessage.emit({ ok: true, __request_id: secondPort.posted[2].__request_id })
    await expect(heartbeat).resolves.toMatchObject({ ok: true })
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
    await expect(first).rejects.toThrow('下载器连接已断开')
    expect(disconnected).toHaveBeenCalledOnce()
    const second = bridge.request({ op: 'ping' })
    secondPort.onMessage.emit({ ok: true, __request_id: secondPort.posted[0].__request_id })
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
    port.onMessage.emit({ ok: true, handoff: { status: 'accepted' }, __request_id: port.posted[0].__request_id })
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
    const timedOutAssertion = expect(timedOut).rejects.toThrow('插件请求超时')
    await vi.advanceTimersByTimeAsync(101)
    await timedOutAssertion
    const active = bridge.request({ op: 'ping' })
    firstPort.onMessage.emit({ ok: true, value: 'stale' })
    secondPort.onMessage.emit({ ok: true, value: 'fresh', __request_id: secondPort.posted[0].__request_id })
    await expect(active).resolves.toMatchObject({ value: 'fresh' })
    bridge.close()
    vi.useRealTimers()
  })

  it('ignores a response without the active v7 request id', async () => {
    const port = new FakePort()
    const bridge = new NativeBridge(() => port)
    const request = bridge.request({ op: 'ping' })

    port.onMessage.emit({ ok: true, value: 'unsolicited' })
    expect(port.posted).toHaveLength(1)
    port.onMessage.emit({
      ok: true,
      value: 'matched',
      __request_id: port.posted[0].__request_id,
    })
    await expect(request).resolves.toMatchObject({ value: 'matched' })
    bridge.close()
  })

  it('disconnects a port whose initial postMessage throws before retrying', async () => {
    vi.useFakeTimers()
    const failedPort = new FakePort()
    failedPort.postMessage = vi.fn(() => { throw new Error('host exited') })
    const replacement = new FakePort()
    const bridge = new NativeBridge(vi.fn()
      .mockReturnValueOnce(failedPort)
      .mockReturnValueOnce(replacement))

    const request = bridge.request({ op: 'ping' }, 30_000, 1)
    expect(failedPort.disconnect).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(81)
    replacement.onMessage.emit({
      ok: true,
      __request_id: replacement.posted[0].__request_id,
    })
    await expect(request).resolves.toMatchObject({ ok: true })
    bridge.close()
    vi.useRealTimers()
  })
})
