import { describe, expect, it } from 'vitest'
import { copyHttpBufferSource, inheritHttpBufferSource } from './blobOwnership'

describe('blob download ownership', () => {
  it('recovers the HTTP source wrapped by new Blob([fetchedBytes])', () => {
    const sources = new WeakMap<object, string>()
    const bytes = new Uint8Array([1, 2, 3])
    sources.set(bytes.buffer, 'https://cdn.test/export.zip')

    expect(inheritHttpBufferSource([bytes], value => sources.get(value))).toBe('https://cdn.test/export.zip')
  })

  it('never invents a source for generated or non-HTTP parts', () => {
    const sources = new WeakMap<object, string>()
    const bytes = new Uint8Array([9])
    sources.set(bytes.buffer, 'data:text/plain,generated')

    expect(inheritHttpBufferSource([bytes], value => sources.get(value))).toBe('')
    expect(inheritHttpBufferSource(['hello'], value => sources.get(value))).toBe('')
    expect(inheritHttpBufferSource(undefined, value => sources.get(value))).toBe('')
  })

  it('copies HTTP ownership onto a sliced Blob', () => {
    const sources = new WeakMap<object, string>()
    const original = { id: 'blob' }
    const sliced = { id: 'slice' }
    sources.set(original, 'https://cdn.test/export.zip')
    copyHttpBufferSource(original, sliced, value => sources.get(value), (value, source) => sources.set(value, source))
    expect(sources.get(sliced)).toBe('https://cdn.test/export.zip')
  })
})
