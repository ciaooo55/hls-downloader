import { describe, expect, it } from 'vitest'
import { taskContextActions } from './taskContextActions'

const task = (status: string, output_path = '') => ({ id: status, status, output_path })

describe('taskContextActions', () => {
  it('offers direct controls for an active task', () => {
    expect(taskContextActions(task('downloading_segments'))).toEqual([
      'details', 'pause', 'cancel', 'copyUrl', 'log', 'delete', 'deleteFiles',
    ])
  })

  it('offers retry and deletion for a failed task', () => {
    expect(taskContextActions(task('failed'))).toEqual([
      'details', 'retry', 'copyUrl', 'log', 'delete', 'deleteFiles',
    ])
  })

  it('offers file access for a completed task', () => {
    expect(taskContextActions(task('done', 'video.mp4'))).toEqual([
      'details', 'open', 'copyUrl', 'log', 'delete', 'deleteFiles',
    ])
  })

  it('puts built-in playback directly in the context menu when ready', () => {
    expect(taskContextActions({
      id: 'playing',
      status: 'downloading_segments',
      available_actions: ['pause', 'cancel', 'preview', 'log', 'delete', 'delete_files'],
    })).toEqual(['details', 'preview', 'pause', 'cancel', 'copyUrl', 'log', 'delete', 'deleteFiles'])
  })

  it('offers shared operations for a multi-selection without single-item actions', () => {
    expect(taskContextActions([task('done', 'one.zip'), task('done', 'two.exe')])).toEqual([
      'delete', 'deleteFiles',
    ])
  })
})
