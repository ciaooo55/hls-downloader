import { describe, expect, it, beforeEach } from 'vitest'
import {
  playCompletionChime,
  resetCompletionSoundState,
  setCompletionSoundEnabled,
  shouldPlayCompletionSound,
} from './completionSound'

beforeEach(() => {
  resetCompletionSoundState()
})

describe('completion sound policy', () => {
  it('is silent until the user opts in', () => {
    expect(shouldPlayCompletionSound(1_000)).toBe(false)
    expect(playCompletionChime(false, 1_000, () => { throw new Error('must not play') })).toBe(false)
  })

  it('plays once when enabled', () => {
    const played: number[] = []
    setCompletionSoundEnabled(true)
    expect(playCompletionChime(false, 2_000, () => { played.push(2_000) })).toBe(true)
    expect(played).toEqual([2_000])
  })

  it('coalesces a burst of completions into one chime', () => {
    const played: number[] = []
    setCompletionSoundEnabled(true)
    expect(playCompletionChime(false, 3_000, () => { played.push(3_000) })).toBe(true)
    expect(playCompletionChime(false, 3_400, () => { played.push(3_400) })).toBe(false)
    expect(playCompletionChime(false, 3_699, () => { played.push(3_699) })).toBe(false)
    expect(playCompletionChime(false, 3_700, () => { played.push(3_700) })).toBe(true)
    expect(played).toEqual([3_000, 3_700])
  })

  it('lets settings preview ignore the enabled flag and coalesce window', () => {
    const played: number[] = []
    expect(playCompletionChime(true, 4_000, () => { played.push(1) })).toBe(true)
    expect(playCompletionChime(true, 4_010, () => { played.push(2) })).toBe(true)
    expect(played).toEqual([1, 2])
  })
})
