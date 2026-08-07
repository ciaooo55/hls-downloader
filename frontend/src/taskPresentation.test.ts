import { describe, expect, it } from 'vitest'
import { filterAndSortTasks, stageLabel, statusLabel, taskSizeSummary } from './taskPresentation'

const task = (id: string, status: string, createdAt: string, title = id) => ({
  id,
  status,
  created_at: createdAt,
  updated_at: createdAt,
  title,
  filename: title,
  url: `https://example.test/${id}.m3u8`,
})

describe('task presentation', () => {
  it('keeps active tasks stable and sorts by creation time', () => {
    const tasks = [
      task('old', 'downloading_segments', '2026-01-01T00:00:00'),
      task('new', 'downloading_segments', '2026-01-01T00:00:01'),
      task('done', 'done', '2026-01-01T00:00:02'),
    ]

    expect(filterAndSortTasks(tasks, 'all', '').map(item => item.id)).toEqual(['new', 'old', 'done'])
  })

  it('searches names, urls, ids and error codes', () => {
    const failed = { ...task('task-403', 'failed', '2026-01-01T00:00:00', 'video'), error_code: 'HTTP_403' }
    expect(filterAndSortTasks([failed], 'all', '403')).toHaveLength(1)
    expect(filterAndSortTasks([failed], 'all', 'missing')).toHaveLength(0)
  })

  it('localizes internal status and stage names', () => {
    expect(statusLabel('downloading_segments')).toBe('下载分片')
    expect(stageLabel('merging')).toBe('合并视频')
  })

  it('keeps downloaded and total size visible for completed, live, and post-processing tasks', () => {
    expect(taskSizeSummary({ downloaded_bytes: 8 * 1024 * 1024, total_bytes: 8 * 1024 * 1024 })).toBe('已下载 8.0 MB · 总大小 8.0 MB')
    expect(taskSizeSummary({ downloaded_bytes: 3 * 1024 * 1024, total_bytes: 0, is_live: true })).toBe('已下载 3.0 MB · 总大小 未知（直播）')
    expect(taskSizeSummary({ downloaded_bytes: 0, total_bytes: 0 })).toBe('已下载 0 B · 总大小 未知')
  })

  it('filters by simplified file category', () => {
    const tasks = [
      { ...task('video', 'done', '2026-01-01T00:00:00'), filename: 'video.mp4', task_type: 'http' },
      { ...task('setup', 'done', '2026-01-01T00:00:01'), filename: 'setup.exe', task_type: 'http' },
    ]
    expect(filterAndSortTasks(tasks, 'media', '').map(item => item.id)).toEqual(['video'])
    expect(filterAndSortTasks(tasks, 'program', '').map(item => item.id)).toEqual(['setup'])
  })

  it('filters failed and unsupported tasks together', () => {
    const tasks = [
      task('a', 'failed', '2026-01-01T00:00:00'),
      task('b', 'unsupported', '2026-01-01T00:00:01'),
      task('c', 'done', '2026-01-01T00:00:02'),
    ]
    expect(filterAndSortTasks(tasks, 'failed', '').map(item => item.id).sort()).toEqual(['a', 'b'])
  })
})
