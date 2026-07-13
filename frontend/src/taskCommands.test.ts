import { describe, expect, it } from 'vitest'
import { commandState } from './taskCommands'

const task = (status: string) => ({ id: status, status })

describe('commandState', () => {
  it('disables task commands without a selection', () => {
    expect(commandState([])).toEqual({
      start: false, pause: false, resume: false, cancel: false,
      retry: false, delete: false, open: false, log: false,
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
  })
})
