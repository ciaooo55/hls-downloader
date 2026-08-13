import { describe, expect, it } from 'vitest'
import {
  clampHudPosition,
  clampSeekSeconds,
  downloadControls,
  downloadPercent,
  emptyCastPlayback,
  mergeCastPlayback,
  playbackPercent,
  livePlaybackPosition,
  relativeSeekTarget,
  shareKindLabel,
  canControlTransport,
} from './castSession'

describe('cast session helpers', () => {
  it('labels share kinds and only casts have transport', () => {
    expect(shareKindLabel('cast')).toBe('投屏播放')
    expect(shareKindLabel('tvbox')).toBe('TVBox 推送')
    expect(canControlTransport('cast')).toBe(true)
    expect(canControlTransport('tvbox')).toBe(false)
  })

  it('clamps seek and percent against duration', () => {
    expect(clampSeekSeconds(-4, 120)).toBe(0)
    expect(clampSeekSeconds(130, 120)).toBe(120)
    expect(relativeSeekTarget(8, -10, 120)).toBe(0)
    expect(relativeSeekTarget(8, 10, 120)).toBe(18)
    expect(playbackPercent(30, 120)).toBe(25)
    expect(playbackPercent(0, 0)).toBe(0)
  })

  it('merges playback snapshots without losing the device label', () => {
    const merged = mergeCastPlayback(emptyCastPlayback(), {
      label: '客厅电视',
      playing: true,
      paused: false,
      position: 12,
      duration: 90,
    })
    expect(merged.label).toBe('客厅电视')
    expect(merged.playing).toBe(true)
    expect(merged.position).toBe(12)
  })

  it('exposes download pause from the live task', () => {
    expect(downloadPercent({ downloaded_bytes: 50, total_bytes: 100 })).toBe(50)
    expect(downloadControls({ id: 't1', status: 'downloading', available_actions: ['pause'] })).toEqual({ pause: true, resume: false })
    expect(downloadControls({ id: 't1', status: 'paused', available_actions: ['resume'] })).toEqual({ pause: false, resume: true })
    expect(downloadControls(null)).toEqual({ pause: false, resume: false })
  })

  it('keeps the HUD inside the viewport', () => {
    expect(clampHudPosition(-40, 900, 320, 120, 800, 600, 12)).toEqual({ left: 12, top: 468 })
  })

  it('advances the HUD clock between status polls while playing', () => {
    const playing = { playing: true, paused: false, position: 10, duration: 90 }
    expect(livePlaybackPosition(playing, 1_000, 3_400, null)).toBe(12)
    expect(livePlaybackPosition(playing, 1_000, 3_400, 40)).toBe(40)
    expect(livePlaybackPosition({ ...playing, paused: true, playing: false }, 1_000, 9_000, null)).toBe(10)
  })
})
