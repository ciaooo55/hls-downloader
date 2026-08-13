import { describe, expect, it } from 'vitest'

import {
  getDisplayedProgress,
  isActiveTransfer,
  isPausable,
  mergeTaskEvent,
  mergeTaskEvents,
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

  it('does not revive a deleted task from a late progress event', () => {
    const tombstones = new Set(['gone'])
    const updated = mergeTaskEvent([], {
      type: 'task_progress', task_id: 'gone', status: 'downloading', completed_segments: 4,
    }, tombstones)
    expect(updated).toEqual([])
  })

  it('merges a progress batch with one pass and keeps the newest fields', () => {
    const tasks = [
      { id: 'one', status: 'downloading', downloaded_bytes: 1, title: 'One' },
      { id: 'two', status: 'queued', downloaded_bytes: 0, title: 'Two' },
    ]
    const merged = mergeTaskEvents(tasks, [
      { type: 'task_progress', task_id: 'one', downloaded_bytes: 2 },
      { type: 'task_progress', task_id: 'two', status: 'downloading' },
      { type: 'task_progress', task_id: 'one', downloaded_bytes: 3 },
    ])
    expect(merged).toEqual([
      { id: 'one', status: 'downloading', downloaded_bytes: 3, title: 'One' },
      { id: 'two', status: 'downloading', downloaded_bytes: 0, title: 'Two' },
    ])
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

  it('hides leftover transfer rates after pause or completion', () => {
    expect(isActiveTransfer('downloading')).toBe(true)
    expect(isActiveTransfer('paused')).toBe(false)
    expect(isActiveTransfer('queued')).toBe(false)
    expect(isActiveTransfer('done')).toBe(false)
  })

  it('falls back to byte progress when percent has not been published', () => {
    expect(getDisplayedProgress({
      status: 'downloading',
      downloaded_bytes: 25,
      total_bytes: 100,
    })).toBe(25)
  })
})
