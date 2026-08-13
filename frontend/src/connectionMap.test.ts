import { describe, expect, it } from 'vitest'
import { buildConnectionMap, normalizeConnectionParts } from './connectionMap'

describe('connection map model', () => {
  it('stays empty without ranged parts', () => {
    expect(normalizeConnectionParts(undefined)).toEqual([])
    expect(buildConnectionMap([], 100)).toBeNull()
  })

  it('sanitizes and paints HTTP ranges', () => {
    const model = buildConnectionMap([
      { start: 0, end: 9, done: 10, state: 'done' },
      { start: 10, end: 19, done: 4, state: 'active' },
      { start: 20, end: 29, done: 0, state: 'queued' },
      { start: 'x', end: 29, done: -2, state: 'nope' },
    ], 30)
    expect(model?.parts).toHaveLength(3)
    expect(model?.active).toBe(1)
    expect(model?.parts[0].fill).toBe(100)
    expect(model?.parts[1].fill).toBe(40)
    expect(model?.parts[2].state).toBe('queued')
  })
})
