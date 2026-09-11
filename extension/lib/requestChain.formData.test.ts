import { describe, expect, it } from 'vitest'
import { captureReplayableRequestBody } from './requestChain'

describe('form-data POST replay capture', () => {
  it('reconstructs a small form body exactly', () => {
    const body = captureReplayableRequestBody({
      formData: {
        asset: ['episode-12'],
        quality: ['1080p', '720p'],
      },
    })

    expect(atob(body)).toBe('asset=episode-12&quality=1080p&quality=720p')
  })

  it('rejects form bodies with too many fields instead of truncating them', () => {
    const fields: Record<string, string[]> = {}
    for (let index = 0; index < 129; index += 1) fields[`field-${index}`] = ['value']

    expect(captureReplayableRequestBody({ formData: fields })).toBe('')
  })

  it('rejects fields with too many values instead of truncating them', () => {
    expect(captureReplayableRequestBody({
      formData: {
        quality: Array.from({ length: 129 }, (_, index) => String(index)),
      },
    })).toBe('')
  })
})
