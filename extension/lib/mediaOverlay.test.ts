import { describe, expect, it } from 'vitest'
import { clampOverlayPosition, shouldShowMediaOverlay } from './mediaOverlay'

describe('media overlay visibility', () => {
  it('stays absent before playback or without associated resources', () => {
    expect(shouldShowMediaOverlay({ hasPlayback: false, hasActiveVideo: true, resourceCount: 1 })).toBe(false)
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: false, resourceCount: 1 })).toBe(false)
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: 0 })).toBe(false)
  })

  it('shows only for a played video with a matching candidate', () => {
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: 1 })).toBe(true)
  })
})

describe('media overlay position', () => {
  it('keeps the entire overlay inside a small viewport', () => {
    expect(clampOverlayPosition({ x: 960, y: -25 }, { width: 344, height: 480 }, { width: 800, height: 600 }))
      .toEqual({ x: 446, y: 10 })
  })

  it('uses the requested margin when the overlay is larger than the viewport', () => {
    expect(clampOverlayPosition({ x: 50, y: 50 }, { width: 900, height: 700 }, { width: 800, height: 600 }))
      .toEqual({ x: 10, y: 10 })
  })
})
