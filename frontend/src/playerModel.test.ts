import { describe, expect, it } from 'vitest'

import {
  formatPlayerTime,
  thumbnailBucket,
  thumbnailLeft,
  timelineTime,
} from './playerModel'

describe('player timeline model', () => {
  it('maps and clamps pointer positions to playable time', () => {
    expect(timelineTime(150, 100, 200, 120)).toBe(30)
    expect(timelineTime(20, 100, 200, 120)).toBe(0)
    expect(timelineTime(400, 100, 200, 120)).toBe(120)
  })

  it('uses sparse thumbnail buckets and keeps previews inside the track', () => {
    expect(thumbnailBucket(27.4, 100)).toBe(20)
    expect(thumbnailLeft(0, 100, 500)).toBe(92)
    expect(thumbnailLeft(100, 100, 500)).toBe(408)
  })

  it('formats short and long media durations', () => {
    expect(formatPlayerTime(65)).toBe('1:05')
    expect(formatPlayerTime(3723)).toBe('1:02:03')
  })
})
