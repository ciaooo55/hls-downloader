import { describe, expect, it } from 'vitest'
import { SEND_LABELS, SEND_LOCKED, SEND_TITLES, sendClickAction, sendStateFromHandoff, type SendState } from './downloadSendState'

describe('sendStateFromHandoff', () => {
  it('maps Core handoff and task facts to one visible state', () => {
    expect(sendStateFromHandoff({ status: 'pending' })).toBe('confirming')
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
})
