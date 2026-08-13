import { describe, expect, it } from 'vitest'
import { duplicateActionLabel, parseDuplicateError, parseDuplicateMatches } from './duplicateTask'

describe('duplicate task reuse', () => {
  it('reads suggested actions from a 409 payload', () => {
    const parsed = parseDuplicateError({
      message: 'x',
      detail: {
        code: 'DUPLICATE_URL',
        message: 'already',
        duplicates: [{ id: 'dup1', status: 'paused', filename: 'a.bin', suggested_action: 'resume' }],
      },
    })
    expect(parsed.message).toBe('already')
    expect(parsed.duplicates[0].suggested_action).toBe('resume')
    expect(duplicateActionLabel('resume')).toContain('\u5df2\u6709')
  })

  it('ignores junk rows', () => {
    expect(parseDuplicateMatches({ duplicates: [{ filename: 'no-id' }, 'x'] })).toEqual([])
  })
})
