import { describe, expect, it } from 'vitest'
import { taskContextActions } from './taskContextActions'

const task = (status: string, output_path = '') => ({ id: status, status, output_path })

describe('taskContextActions', () => {
  it('offers direct controls for an active task', () => {
    expect(taskContextActions(task('downloading_segments'))).toEqual([
      'details', 'pause', 'cancel', 'log',
    ])
  })

  it('offers retry and deletion for a failed task', () => {
    expect(taskContextActions(task('failed'))).toEqual([
      'details', 'retry', 'log', 'delete',
    ])
  })

  it('offers file access for a completed task', () => {
    expect(taskContextActions(task('done', 'video.mp4'))).toEqual([
      'details', 'open', 'log', 'delete',
    ])
  })
})
