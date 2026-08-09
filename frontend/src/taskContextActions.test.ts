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
      'details', 'open', 'cast', 'pushTvbox', 'copyUrl', 'log', 'delete', 'deleteFiles',
    ])
  })

  it('only exposes media delivery after a completed local file exists', () => {
    expect(taskContextActions(task('downloading_segments', 'video.mp4'))).not.toContain('cast')
    expect(taskContextActions({ ...task('done', 'collection'), output_is_file: false })).not.toContain('cast')
  })

  it('exposes delivery for an HTTP task once a verified growing range is playable', () => {
    const active = { ...task('downloading', 'video.mp4'), task_type: 'http' as const, playback_ready: true }
    expect(taskContextActions(active)).toContain('cast')
    expect(taskContextActions(active)).toContain('pushTvbox')
  })

  it('exposes delivery for an active HLS task through a local playlist', () => {
    const active = { ...task('downloading_segments', 'live.m3u8'), task_type: 'hls' as const, playback_ready: true }
    expect(taskContextActions(active)).toContain('cast')
    expect(taskContextActions(active)).toContain('pushTvbox')
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
