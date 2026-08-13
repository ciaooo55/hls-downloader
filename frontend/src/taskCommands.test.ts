import { describe, expect, it } from 'vitest'
import { commandState, pauseLabelFor, resumeLabelFor } from './taskCommands'

const task = (status: string) => ({ id: status, status })

describe('commandState', () => {
  it('disables task commands without a selection', () => {
    expect(commandState([])).toEqual({
      start: false, pause: false, resume: false, cancel: false,
      retry: false, delete: false, open: false, log: false,
      pauseLabel: '暂停', resumeLabel: '恢复',
    })
  })

  it('enables pause only when every selected task is pausable', () => {
    expect(commandState([task('downloading_segments')]).pause).toBe(true)
    expect(commandState([task('downloading_segments'), task('done')]).pause).toBe(false)
  })

  it('maps terminal and paused states to valid commands', () => {
    expect(commandState([task('paused')]).resume).toBe(true)
    expect(commandState([task('failed')]).retry).toBe(true)
    expect(commandState([task('done')]).open).toBe(true)
    expect(commandState([task('done')]).delete).toBe(true)
    expect(commandState([{ id: 'missing', status: 'done', output_missing: true }]).retry).toBe(true)
  })

  it('uses backend action flags instead of guessing from status', () => {
    expect(commandState([{ id: 'queued', status: 'queued', available_actions: ['cancel', 'log'] }]).start).toBe(false)
    expect(commandState([{ id: 'parsing', status: 'parsing', available_actions: ['cancel', 'log'] }]).pause).toBe(false)
    expect(commandState([{ id: 'segments', status: 'downloading_segments', available_actions: ['pause', 'cancel', 'log'] }]).pause).toBe(true)
  })

  it('labels pause as stop-recording only when live tasks are targeted', () => {
    const live = { id: 'live', status: 'downloading_segments', is_live: true }
    const vod = { id: 'vod', status: 'downloading_segments', is_live: false }
    expect(pauseLabelFor([vod])).toBe('暂停')
    expect(pauseLabelFor([live])).toBe('停止录制')
    // Mixed selections must warn about both outcomes: stopping a live
    // recording is irreversible while pausing a VOD download is not.
    expect(pauseLabelFor([live, vod])).toBe('暂停 / 停止录制')
    expect(resumeLabelFor([live])).toBe('继续录制')
    expect(resumeLabelFor([live, vod])).toBe('恢复 / 继续录制')
    expect(commandState([live]).pauseLabel).toBe('停止录制')
    expect(commandState([live, vod]).pauseLabel).toBe('暂停 / 停止录制')
  })
})
