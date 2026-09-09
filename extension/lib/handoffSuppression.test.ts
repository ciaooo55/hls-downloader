import { describe, expect, it } from 'vitest'
import { isHandoffSuppressed, normalizeHandoffSuppressions } from './handoffSuppression'

describe('automatic handoff suppressions', () => {
  it('matches only the source-page host and exact resource kind', () => {
    const rules = normalizeHandoffSuppressions([
      { host: 'video.example.test', kind: 'hls' },
    ])

    expect(isHandoffSuppressed(rules, 'https://video.example.test/watch/42', 'hls')).toBe(true)
    expect(isHandoffSuppressed(rules, 'https://video.example.test/watch/42', 'media')).toBe(false)
    expect(isHandoffSuppressed(rules, 'https://cdn.example.test/video.m3u8', 'hls')).toBe(false)
  })

  it('drops malformed and duplicate persisted rules', () => {
    expect(normalizeHandoffSuppressions([
      { host: 'Video.Example.Test', kind: 'hls' },
      { host: 'video.example.test', kind: 'hls' },
      { host: '', kind: 'file' },
      { host: 'video.example.test', kind: 'unknown' },
    ])).toEqual([{ host: 'video.example.test', kind: 'hls' }])
  })
})
