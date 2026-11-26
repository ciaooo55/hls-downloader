import { describe, expect, it, vi } from 'vitest'
import { waitForDownloadConfirmation } from './downloadConfirmation'
import { SEND_LABELS, SEND_LOCKED, SEND_TITLES, canForceResend, sendClickAction, sendStateFromHandoff, type SendState } from './downloadSendState'

describe('sendStateFromHandoff', () => {
  it('maps Core handoff and task facts to one visible state', () => {
    expect(sendStateFromHandoff({ status: 'pending' })).toBe('confirming')
    expect(sendStateFromHandoff({ status: 'pending', presentation: 'workbench' })).toBe('confirming')
    expect(sendStateFromHandoff({ status: 'pending', presentation: 'fallback' })).toBe('retry')
    expect(sendStateFromHandoff({ status: 'pending', presentation: 'failed' })).toBe('retry')
    expect(sendStateFromHandoff({ status: 'accepted', taskStatus: 'queued' })).toBe('queued')
    expect(sendStateFromHandoff({ status: 'accepted', taskStatus: 'downloading' })).toBe('queued')
    expect(sendStateFromHandoff({ status: 'accepted', taskStatus: 'completed' })).toBe('done')
    expect(sendStateFromHandoff({ status: 'accepted', taskStatus: 'failed' })).toBe('recoverable')
    expect(sendStateFromHandoff({ status: 'accepted', taskStatus: 'paused' })).toBe('recoverable')
    expect(sendStateFromHandoff({ status: 'rejected' })).toBe('canceled')
    expect(sendStateFromHandoff({ status: 'canceled' })).toBe('canceled')
    expect(sendStateFromHandoff({ status: 'connection_lost' })).toBe('checking')
    expect(sendStateFromHandoff({ status: 'expired' })).toBe('retry')
  })

  it('never offers a fresh send while Core may still own the request', () => {
    // 回执丢失或 Core 重启期间（checking），点击只回查原请求，不新建交接。
    expect(sendClickAction('checking')).toBe('recheck')
    for (const state of ['sending', 'confirming', 'queued', 'done'] as SendState[]) {
      expect(SEND_LOCKED.has(state)).toBe(true)
      expect(sendClickAction(state)).toBe('none')
    }
    expect(sendClickAction('recoverable')).toBe('open_desktop')
    expect(sendClickAction('canceled')).toBe('send')
    expect(sendClickAction(undefined)).toBe('send')
  })

  it('labels every state', () => {
    for (const state of Object.keys(SEND_LABELS) as SendState[]) {
      expect(SEND_LABELS[state].length).toBeGreaterThan(0)
      expect(SEND_TITLES[state].length).toBeGreaterThan(0)
    }
  })

  it('unlocks refresh/resend for ArmTaskRefresh, URL updates, and explicit resend affordances', () => {
    expect(sendStateFromHandoff({ status: 'accepted', taskStatus: 'paused', taskErrorCode: 'refresh_armed' })).toBe('retry')
    expect(sendClickAction('queued', { urlChanged: true })).toBe('send')
    expect(sendClickAction('done', { urlChanged: true })).toBe('send')
    expect(sendClickAction('recoverable', { cooldownElapsed: true })).toBe('send')
    expect(canForceResend('queued')).toBe(true)
    expect(canForceResend('done')).toBe(true)
    expect(canForceResend('recoverable')).toBe(true)
    expect(canForceResend('sending')).toBe(false)
    expect(canForceResend('confirming')).toBe(false)
  })

  it('keeps waiting across Workbench TaskEditorWindow presentation until user confirms or cancels', async () => {
    vi.useFakeTimers()
    try {
      const send = vi.fn()
        .mockResolvedValueOnce({ ok: true, handoff: { status: 'pending', presentation: 'presenting' } })
        .mockResolvedValueOnce({ ok: true, handoff: { status: 'pending', presentation: 'workbench' } })
        .mockResolvedValueOnce({ ok: true, handoff: { status: 'accepted' } })
      const pending = waitForDownloadConfirmation(send, 'handoff-more-settings')
      await vi.advanceTimersByTimeAsync(1_300)
      await expect(pending).resolves.toBe(true)
      expect(send).toHaveBeenCalledTimes(3)

      const failedSend = vi.fn().mockResolvedValue({
        ok: true,
        handoff: { status: 'pending', presentation: 'failed', presentation_error: '确认窗启动失败' },
      })
      await expect(waitForDownloadConfirmation(failedSend, 'handoff-failed')).rejects.toThrow('确认窗启动失败')
    } finally {
      vi.useRealTimers()
    }
  })
})
