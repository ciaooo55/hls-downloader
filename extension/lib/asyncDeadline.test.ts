import { describe, expect, it } from 'vitest'
import { withDeadline } from './asyncDeadline'

describe('async deadline', () => {
  it('returns a fast operation without waiting for the deadline', async () => {
    await expect(withDeadline(Promise.resolve('ok'), 50)).resolves.toBe('ok')
  })

  it('turns a stalled operation into a readable retry state', async () => {
    await expect(withDeadline(new Promise(() => undefined), 5, '连接超时')).rejects.toThrow('连接超时')
  })
})
