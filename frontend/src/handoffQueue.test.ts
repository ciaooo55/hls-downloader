import { describe, expect, it } from 'vitest'
import { HandoffWindowQueue } from './handoffQueue'

describe('HandoffWindowQueue', () => {
  it('shows rapid browser offers one at a time in arrival order', () => {
    const queue = new HandoffWindowQueue()

    expect(queue.enqueue('first')).toBe(true)
    expect(queue.enqueue('second')).toBe(true)
    expect(queue.enqueue('third')).toBe(true)
    expect(queue.begin()).toBe('first')
    expect(queue.activeId).toBe('first')
    expect(queue.release('first')).toBe(true)
    expect(queue.begin()).toBe('second')
    expect(queue.release('second')).toBe(true)
    expect(queue.begin()).toBe('third')
  })

  it('does not duplicate an already visible or queued offer', () => {
    const queue = new HandoffWindowQueue()

    expect(queue.enqueue('one')).toBe(true)
    expect(queue.enqueue('one')).toBe(false)
    expect(queue.begin()).toBe('one')
    expect(queue.enqueue('one')).toBe(false)
    expect(queue.release('other')).toBe(false)
    expect(queue.activeId).toBe('one')
  })
})
