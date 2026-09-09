import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { HOVER_DISMISS_DELAY_MS, clampOverlayPosition, overlayActionFallback, overlayResourceDetails, overlaySendKey, safeResourceLocation, shouldKeepHoverOpen, shouldShowMediaOverlay } from './mediaOverlay'

describe('media overlay visibility', () => {
  it('stays absent before playback', () => {
    expect(shouldShowMediaOverlay({ hasPlayback: false, hasActiveVideo: true, resourceCount: 1 })).toBe(false)
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: false, resourceCount: 1 })).toBe(false)
  })

  it('shows an identifying state immediately for a played video', () => {
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: 0 })).toBe(true)
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: 1 })).toBe(true)
  })
})

describe('manual page rescan', () => {
  it('replays MSE evidence and re-reads media elements without opening the panel', () => {
    const content = readFileSync(fileURLToPath(new URL('../entrypoints/content.ts', import.meta.url)), 'utf8')
    const branch = content.slice(content.indexOf("message?.type === 'rescan-media'"), content.indexOf("message?.type === 'rescan-media'") + 1_300)
    expect(branch).toContain('__hls_downloader_replay__')
    expect(branch).toContain("querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video,audio')")
    expect(branch).toContain("performance.getEntriesByType('resource')")
    expect(branch).not.toContain('setOpen(true)')
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

describe('overlay send keys', () => {
  it('keeps download, TVBox, and cast busy states on separate keys', () => {
    expect(overlaySendKey('fp-1')).toBe('fp-1')
    expect(overlaySendKey('fp-1', 'download')).toBe('fp-1')
    expect(overlaySendKey('fp-1', 'tvbox')).toBe('fp-1:tvbox')
    expect(overlaySendKey('fp-1', 'cast')).toBe('fp-1:cast')
    expect(overlayActionFallback('tvbox')).toBe('TVBox')
    expect(overlayActionFallback('cast')).toBe('投屏')
  })
})

describe('overlay hover details', () => {
  it('keeps the card open while the pointer or keyboard focus moves into it', () => {
    expect(HOVER_DISMISS_DELAY_MS).toBeGreaterThanOrEqual(400)
    expect(shouldKeepHoverOpen(true, false)).toBe(true)
    expect(shouldKeepHoverOpen(false, true)).toBe(true)
    expect(shouldKeepHoverOpen(false, false)).toBe(false)
  })

  it('shows concise decision metadata without exposing signed URL parameters', () => {
    const details = overlayResourceDetails({
      id: 'stream',
      kind: 'hls',
      url: 'https://user:secret@cdn.test/live/master.m3u8?token=private&expires=999',
      title: '演示视频',
      quality: '1080p',
      width: 1920,
      height: 1080,
      bandwidth: 5_200_000,
      duration: 3661,
      estimatedSize: 2_379_720_000,
      inspected: true,
      seenAt: 1,
    })

    expect(details).toEqual({
      title: '演示视频',
      facts: ['HLS', '1080p', '1920×1080', '5.2 Mbps', '1:01:01', '约 2.2 GB'],
      source: 'cdn.test/live/master.m3u8',
      state: '清单已解析',
    })
    expect(JSON.stringify(details)).not.toContain('private')
    expect(JSON.stringify(details)).not.toContain('secret')
  })

  it('keeps the source label useful for opaque and malformed locations', () => {
    expect(safeResourceLocation('https://cdn.test/a/b/video.mp4?signature=secret')).toBe('cdn.test/b/video.mp4')
    expect(safeResourceLocation('not a URL')).toBe('媒体来源')
  })
})
