import { describe, expect, it } from 'vitest'
import {
  applyQueueReorder,
  isQueueReorderDrag,
  isQueuedTask,
  QUEUE_DRAG_MIME,
  queueDropPlacement,
  queueReorderDirection,
} from './queueReorder'

describe('queueReorder', () => {
  it('only treats queued rows as reorderable', () => {
    expect(isQueuedTask({ status: 'queued' })).toBe(true)
    expect(isQueuedTask({ status: 'downloading' })).toBe(false)
    expect(isQueuedTask({ status: 'done' })).toBe(false)
    expect(isQueuedTask({ status: 'awaiting_selection' })).toBe(false)
  })

  it('builds before/after directions and ignores a self drop', () => {
    expect(queueReorderDirection('a', 'b', 'before')).toBe('before:b')
    expect(queueReorderDirection('a', 'b', 'after')).toBe('after:b')
    expect(queueReorderDirection('a', 'a', 'before')).toBeNull()
    expect(queueReorderDirection('', 'b', 'after')).toBeNull()
  })

  it('places the drop above or below the row midpoint', () => {
    expect(queueDropPlacement(10, 0, 40)).toBe('before')
    expect(queueDropPlacement(30, 0, 40)).toBe('after')
  })

  it('recognizes the internal queue drag mime and not file drops', () => {
    expect(isQueueReorderDrag([QUEUE_DRAG_MIME, 'text/plain'])).toBe(true)
    expect(isQueueReorderDrag(['Files', 'text/uri-list'])).toBe(false)
  })

  it('moves a queued task before or after another queued task', () => {
    const tasks = [
      { id: 'a', status: 'queued', queue_position: 1 },
      { id: 'b', status: 'queued', queue_position: 2 },
      { id: 'c', status: 'queued', queue_position: 3 },
      { id: 'done', status: 'done', queue_position: 0 },
    ]
    expect(applyQueueReorder(tasks, 'c', 'a', 'before').map(task => [task.id, task.queue_position])).toEqual([
      ['a', 2],
      ['b', 3],
      ['c', 1],
      ['done', 0],
    ])
    expect(applyQueueReorder(tasks, 'a', 'c', 'after').map(task => [task.id, task.queue_position])).toEqual([
      ['a', 3],
      ['b', 1],
      ['c', 2],
      ['done', 0],
    ])
  })

  it('ignores drops onto non-queued rows', () => {
    const tasks = [
      { id: 'a', status: 'queued', queue_position: 1 },
      { id: 'done', status: 'done', queue_position: 0 },
    ]
    expect(applyQueueReorder(tasks, 'a', 'done', 'before')).toEqual(tasks)
  })
})
