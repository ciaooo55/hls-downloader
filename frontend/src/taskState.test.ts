import { describe, expect, it } from 'vitest'

import {
  getDisplayedProgress,
  isPausable,
  mergeTaskEvent,
} from './taskState'


describe('task state updates', () => {
  it('merges progress events without replacing stable task fields', () => {
    const tasks = [{
      id: 'one',
      title: 'Video',
      status: 'downloading_segments',
      completed_segments: 1,
      total_segments: 10,
    }]

    const updated = mergeTaskEvent(tasks, {
      type: 'task_progress',
      task_id: 'one',
      status: 'failed',
      error_code: 'HTTP_403',
      error_hint: '检查请求头',
    })

    expect(updated[0]).toMatchObject({
      id: 'one',
      title: 'Video',
      status: 'failed',
      error_code: 'HTTP_403',
      error_hint: '检查请求头',
    })
  })

  it('adds full create events and removes deleted tasks', () => {
    const created = mergeTaskEvent([], {
      type: 'task_created',
      id: 'new',
      task_id: 'new',
      title: 'New task',
      status: 'queued',
    })
    expect(created).toHaveLength(1)

    const deleted = mergeTaskEvent(created, {
      type: 'task_deleted',
      task_id: 'new',
    })
    expect(deleted).toEqual([])
  })
})


describe('task progress presentation', () => {
  it('uses post-processing progress while merging', () => {
    expect(getDisplayedProgress({
      status: 'merging',
      completed_segments: 10,
      total_segments: 10,
      post_percent: 35,
    })).toBe(35)
  })

  it('only allows pausing during segment downloads', () => {
    expect(isPausable({ status: 'downloading_segments' })).toBe(true)
    expect(isPausable({ status: 'merging' })).toBe(false)
    expect(isPausable({ status: 'pausing' })).toBe(false)
  })
})
