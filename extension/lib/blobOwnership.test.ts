import { describe, expect, it } from 'vitest'
import { copyHttpBufferSource, inheritHttpBufferSource } from './blobOwnership'

describe('blob download ownership', () => {
  it('recovers the HTTP source wrapped by new Blob([fetchedBytes])', () => {
    const sources = new WeakMap<object, string>()
    const bytes = new Uint8Array([1, 2, 3])
    sources.set(bytes.buffer, 'https://cdn.test/export.zip')

    expect(inheritHttpBufferSource([bytes], value => sources.get(value))).toBe('https://cdn.test/export.zip')
  })

  it('rejects partial views that cover only part of an owned backing buffer', () => {
    const sources = new WeakMap<object, string>()
    const buffer = new Uint8Array([1, 2, 3, 4]).buffer
    const full = new Uint8Array(buffer)
    const partial = new Uint8Array(buffer, 1, 2)
    sources.set(buffer, 'https://cdn.test/export.zip')

    expect(inheritHttpBufferSource([full], value => sources.get(value)))
      .toBe('https://cdn.test/export.zip')
    expect(inheritHttpBufferSource([partial], value => sources.get(value))).toBe('')
  })

  it('keeps ownership only for one non-empty HTTP-backed part', () => {
    const sources = new WeakMap<object, string>()
    const first = new Uint8Array([1, 2])
    const second = new Uint8Array([3, 4])
    const other = new Uint8Array([5])
    sources.set(first.buffer, 'https://cdn.test/export.zip')
    sources.set(second.buffer, 'https://cdn.test/export.zip')
    sources.set(other.buffer, 'https://cdn.test/other.zip')

    expect(inheritHttpBufferSource([first, second], value => sources.get(value))).toBe('')
    expect(inheritHttpBufferSource([first, new Uint8Array(0)], value => sources.get(value)))
      .toBe('https://cdn.test/export.zip')
    expect(inheritHttpBufferSource([first, other], value => sources.get(value))).toBe('')
    expect(inheritHttpBufferSource([first, 'generated footer'], value => sources.get(value))).toBe('')
    expect(inheritHttpBufferSource([first, new Uint8Array([9])], value => sources.get(value))).toBe('')
  })

  it('never invents a source for generated or non-HTTP parts', () => {
    const sources = new WeakMap<object, string>()
    const bytes = new Uint8Array([9])
    sources.set(bytes.buffer, 'data:text/plain,generated')

    expect(inheritHttpBufferSource([bytes], value => sources.get(value))).toBe('')
    expect(inheritHttpBufferSource(['hello'], value => sources.get(value))).toBe('')
    expect(inheritHttpBufferSource(undefined, value => sources.get(value))).toBe('')
  })

  it('copies ownership only when a derived Blob keeps the complete byte payload', () => {
    const sources = new WeakMap<object, string>()
    const original = { id: 'blob', size: 4 }
    const retagged = { id: 'retagged', size: 4 }
    const partial = { id: 'partial', size: 2 }
    sources.set(original, 'https://cdn.test/export.zip')

    copyHttpBufferSource(original, retagged, value => sources.get(value), (value, source) => sources.set(value, source))
    copyHttpBufferSource(original, partial, value => sources.get(value), (value, source) => sources.set(value, source))

    expect(sources.get(retagged)).toBe('https://cdn.test/export.zip')
    expect(sources.get(partial)).toBeUndefined()
  })
})
