import { afterEach, describe, expect, it, vi } from 'vitest'
import { createHandoffHostReady } from './handoffHostReady'

afterEach(() => {
  vi.useRealTimers()
})

describe('createHandoffHostReady', () => {
  it('resolves immediately after the host has already signalled ready', async () => {
    const gate = createHandoffHostReady(1_000)
    gate.markReady()
    await expect(gate.wait()).resolves.toBeUndefined()
  })

  it('does not treat a short timeout as ready', async () => {
    vi.useFakeTimers()
    const gate = createHandoffHostReady(3_000)
    const pending = gate.wait()
    let settled = false
    void pending.then(() => { settled = true }, () => { settled = true })
    await vi.advanceTimersByTimeAsync(2_999)
    expect(settled).toBe(false)
    expect(gate.ready).toBe(false)
    gate.markReady()
    await expect(pending).resolves.toBeUndefined()
  })

  it('rejects when the host never becomes ready', async () => {
    vi.useFakeTimers()
    const gate = createHandoffHostReady(3_000)
    const pending = gate.wait()
    const expectation = expect(pending).rejects.toThrow('下载确认窗口未就绪')
    await vi.advanceTimersByTimeAsync(3_000)
    await expectation
  })
})
