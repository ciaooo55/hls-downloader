import { describe, expect, it } from 'vitest'
import { suggestedResourceFilename } from './resources'

describe('resource filename bounds', () => {
  it('does not split a surrogate pair at the suggested filename limit', () => {
    const prefix = 'a'.repeat(199)
    const result = suggestedResourceFilename({
      kind: 'media',
      url: 'https://cdn.test/movie.mp4',
      filename: `${prefix}😀.mp4`,
    })

    expect(result).toBe(prefix)
    expect(/[\uD800-\uDBFF]$/.test(result)).toBe(false)
  })
})
