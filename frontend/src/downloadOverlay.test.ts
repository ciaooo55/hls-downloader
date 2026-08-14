import { describe, expect, it } from 'vitest'
import { commandState } from './taskCommands'
import {
  dismissCompleteItem,
  enqueueCompleteItem,
  needsExecutableConfirm,
  progressWindowHeight,
  pruneDismissedProgressIds,
  selectProgressTasks,
  shouldShowProgressWindow,
  toCompleteItem,
  toProgressItem,
  COMPLETE_QUEUE_CAP,
  PROGRESS_CHROME_HEIGHT,
  PROGRESS_MAX_VISIBLE,
  PROGRESS_PAD,
  PROGRESS_ROW_HEIGHT,
} from './downloadOverlay'

describe('download overlay selection', () => {
  it('keeps the small window on running and post-processing tasks only', () => {
    const tasks = [
      { id: 'a', status: 'downloading_segments' },
      { id: 'b', status: 'queued' },
      { id: 'c', status: 'paused' },
      { id: 'd', status: 'merging' },
      { id: 'e', status: 'done' },
      { id: 'f', status: 'failed' },
    ]
    expect(selectProgressTasks(tasks).map(task => task.id)).toEqual(['a', 'd'])
  })

  it('does not reopen after the user closes it until a new running task appears', () => {
    expect(shouldShowProgressWindow(['a', 'b'], new Set(['a', 'b']))).toBe(false)
    expect(shouldShowProgressWindow(['a', 'b', 'c'], new Set(['a', 'b']))).toBe(true)
    expect(shouldShowProgressWindow([], new Set(['a']))).toBe(false)
    expect([...pruneDismissedProgressIds(new Set(['a', 'gone']), ['a'])]).toEqual(['a'])
  })

  it('sizes the progress window by visible rows', () => {
    expect(progressWindowHeight(0)).toBe(PROGRESS_CHROME_HEIGHT + PROGRESS_PAD + PROGRESS_ROW_HEIGHT)
    expect(progressWindowHeight(1)).toBe(PROGRESS_CHROME_HEIGHT + PROGRESS_PAD + PROGRESS_ROW_HEIGHT)
    expect(progressWindowHeight(PROGRESS_MAX_VISIBLE + 3)).toBe(
      PROGRESS_CHROME_HEIGHT + PROGRESS_PAD + PROGRESS_MAX_VISIBLE * PROGRESS_ROW_HEIGHT,
    )
  })
})

describe('download complete popup queue', () => {
  it('maps a finished task onto the popup payload', () => {
    expect(toCompleteItem({
      task_id: 'done-1',
      title: '电影',
      filename: 'movie.mp4',
      output_path: 'D:\\Downloads\\movie.mp4',
      downloaded_bytes: 4096,
      output_is_file: true,
    })).toEqual({
      id: 'done-1',
      title: '电影',
      filename: 'movie.mp4',
      output_path: 'D:\\Downloads\\movie.mp4',
      downloaded_bytes: 4096,
      output_is_file: true,
    })
  })

  it('queues unique completions and drops the oldest past the cap', () => {
    const first = toCompleteItem({ id: '1', filename: 'a.bin' })!
    const updated = toCompleteItem({ id: '1', filename: 'a-renamed.bin' })!
    expect(enqueueCompleteItem([first], updated)).toEqual([updated])

    let queue = [] as ReturnType<typeof toCompleteItem>[]
    for (let index = 1; index <= COMPLETE_QUEUE_CAP + 2; index += 1) {
      queue = enqueueCompleteItem(queue as any, toCompleteItem({ id: String(index), filename: `${index}.bin` })!)
    }
    expect(queue).toHaveLength(COMPLETE_QUEUE_CAP)
    expect(queue[0]?.id).toBe('3')
    expect(dismissCompleteItem(queue as any, '3').map(item => item.id)[0]).toBe('4')
  })

  it('asks before opening an executable from the complete popup', () => {
    expect(needsExecutableConfirm('D:\\Downloads\\setup.exe')).toBe(true)
    expect(needsExecutableConfirm('D:\\Downloads\\movie.mp4')).toBe(false)
  })
})

describe('progress snapshot', () => {
  it('uses displayed progress so merging is not stuck at zero', () => {
    const item = toProgressItem({
      id: 'merge-1',
      filename: 'show.mp4',
      status: 'merging',
      post_percent: 40,
      downloaded_bytes: 100,
      total_bytes: 100,
    })
    expect(item?.progress_percent).toBe(40)
    expect(item?.filename).toBe('show.mp4')
  })

  it('lets pause/cancel fall back to status when the snapshot has no action flags', () => {
    const item = toProgressItem({
      id: 'http-1',
      filename: 'file.bin',
      status: 'downloading_segments',
      progress_percent: 12,
    })
    expect(item?.available_actions).toBeUndefined()
    expect(commandState([item!]).pause).toBe(true)
    expect(commandState([item!]).cancel).toBe(true)
  })
})
