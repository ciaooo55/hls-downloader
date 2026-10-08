import { afterEach, describe, expect, it, vi } from 'vitest'
import { waitForDownloadConfirmation } from './downloadConfirmation'

afterEach(() => vi.useRealTimers())

describe('desktop download confirmation', () => {
  it('keeps waiting until the desktop accepts, then reports acceptance', async () => {
    vi.useFakeTimers()
    const send = vi.fn().mockResolvedValueOnce({ ok: true, handoff: { status: 'pending', presentation: 'shown' } })
      .mockResolvedValueOnce({ ok: true, handoff: { status: 'accepted' } })
    const result = waitForDownloadConfirmation(send, 'offer-1')
    await vi.advanceTimersByTimeAsync(600)
    expect(await result).toBe(true)
  })

  it('returns cancellation without reporting a queued task', async () => {
    expect(await waitForDownloadConfirmation(async () => ({ ok: true, handoff: { status: 'rejected' } }), 'offer-1')).toBe(false)
  })

  it('surfaces connection and presentation failures instead of false success', async () => {
    await expect(waitForDownloadConfirmation(async () => ({ ok: false, error: '引擎断开' }), 'offer-1')).rejects.toThrow('引擎断开')
    await expect(waitForDownloadConfirmation(async () => ({ ok: true, handoff: { status: 'pending', presentation: 'fallback' } }), 'offer-1')).rejects.toThrow('无法显示下载确认窗口')
    await expect(waitForDownloadConfirmation(async () => ({ ok: true, handoff: { status: 'expired' } }), 'offer-1')).rejects.toThrow('下载确认已失效')
  })

  it('releases the button when a desktop confirmation remains unanswered', async () => {
    vi.useFakeTimers()
    const result = waitForDownloadConfirmation(async () => ({ ok: true, handoff: { status: 'pending' } }), 'offer-1')
    const assertion = expect(result).rejects.toThrow('尚未确认下载')
    await vi.advanceTimersByTimeAsync(126_000)
    await assertion
  })
})
