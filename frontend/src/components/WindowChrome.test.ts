import { describe, expect, it } from 'vitest'

import { nextWindowSizeAction } from './WindowChrome'

describe('window chrome maximize action', () => {
  it('uses the capability-backed maximize and unmaximize commands', () => {
    expect(nextWindowSizeAction(false)).toBe('maximize')
    expect(nextWindowSizeAction(true)).toBe('unmaximize')
  })
})
