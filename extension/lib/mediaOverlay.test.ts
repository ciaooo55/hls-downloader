import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { HOVER_DISMISS_DELAY_MS, clampOverlayPosition, mediaElementAtPoint, mediaOverlayAncestors, overlayActionFallback, overlayResourceDetails, overlaySendKey, safeResourceLocation, shouldKeepHoverOpen, shouldShowMediaOverlay, visibleMediaRect } from './mediaOverlay'

describe('media overlay visibility', () => {
  it('stays absent before playback', () => {
    expect(shouldShowMediaOverlay({ hasPlayback: false, hasActiveVideo: true, resourceCount: 1 })).toBe(false)
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: false, resourceCount: 1 })).toBe(false)
  })

  it('waits for a confirmed source instead of showing speculative controls', () => {
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: 0 })).toBe(false)
    expect(shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: 1 })).toBe(true)
  })
})

describe('visible media geometry', () => {
  const viewport = { width: 800, height: 600 }
  function fixture() {
    const styles = new WeakMap<object, Record<string, string>>()
    const document = { visibilityState: 'visible', defaultView: { getComputedStyle: (node: object) => styles.get(node) } }
    const node = (left: number, top: number, width: number, height: number, parent: any = null, style = {}) => {
      const element = { ownerDocument: document, isConnected: true, parentElement: parent,
        getRootNode: () => document, offsetWidth: width, offsetHeight: height,
        clientWidth: width, clientHeight: height, clientLeft: 0, clientTop: 0,
        getBoundingClientRect: () => ({ x: left, y: top, left, top, right: left + width, bottom: top + height, width, height }) }
      styles.set(element, { visibility: 'visible', display: 'block', opacity: '1',
        overflowX: 'visible', overflowY: 'visible', ...style })
      return element as unknown as HTMLElement
    }
    return { node, document }
  }

  it('clips each axis independently through nested overflow containers and the viewport', () => {
    const { node } = fixture()
    const outer = node(100, 0, 300, 600, null, { overflowX: 'hidden' })
    const inner = node(0, 80, 800, 200, outer, { overflowY: 'auto' })
    const video = node(-20, 10, 900, 500, inner)
    expect(visibleMediaRect(video, viewport)).toEqual({
      x: 100, y: 80, left: 100, top: 80, right: 400, bottom: 280, width: 300, height: 200,
    })
    expect(visibleMediaRect(node(500, 10, 200, 100, outer), viewport)).toBeNull()
    expect(visibleMediaRect(node(0, 600, 200, 100), viewport)).toBeNull()
  })

  it.each([{ display: 'none' }, { opacity: '0' }, { contentVisibility: 'hidden' }])('rejects hidden ancestors: %j', style => {
    const { node } = fixture()
    expect(visibleMediaRect(node(0, 0, 400, 300, node(0, 0, 400, 300, null, style)), viewport)).toBeNull()
  })

  it('rejects hidden media, inactive documents and removed nodes without rejecting cover or portrait video', () => {
    const { node, document } = fixture()
    expect(visibleMediaRect(node(0, 0, 200, 400, null, { visibility: 'hidden' }), viewport)).toBeNull()
    const video = node(0, 0, 200, 400, null, { objectFit: 'cover' })
    expect(visibleMediaRect(video, viewport)?.height).toBe(400)
    document.visibilityState = 'hidden'
    expect(visibleMediaRect(video, viewport)).toBeNull()
    document.visibilityState = 'visible'
    Object.assign(video, { isConnected: false })
    expect(visibleMediaRect(video, viewport)).toBeNull()
  })

  it('uses the scaled client box rather than borders and scrollbars as clipping bounds', () => {
    const { node } = fixture()
    const parent = node(100, 50, 400, 300, null, { overflowX: 'scroll', overflowY: 'clip' })
    Object.assign(parent, { offsetWidth: 200, offsetHeight: 150, clientLeft: 5, clientTop: 5, clientWidth: 180, clientHeight: 130 })
    const rect = visibleMediaRect(node(0, 0, 800, 600, parent), viewport)
    expect(rect).toMatchObject({ left: 110, top: 60, right: 470, bottom: 320 })
  })

  it('crosses shadow roots to include host clipping', () => {
    const { node } = fixture()
    const host = node(100, 100, 250, 200, null, { overflowX: 'hidden', overflowY: 'hidden' })
    const video = node(0, 0, 600, 400)
    Object.assign(video, { getRootNode: () => ({ host }) })
    expect(visibleMediaRect(video, viewport)).toMatchObject({ left: 100, top: 100, width: 250, height: 200 })
  })

  it('follows assigned slots through their shadow containers instead of skipping to the light-DOM host', () => {
    const { node } = fixture()
    const host = node(0, 0, 800, 600)
    const container = node(120, 90, 240, 180, null, { overflowX: 'hidden', overflowY: 'auto' })
    Object.assign(container, { getRootNode: () => ({ host }) })
    const slot = node(0, 0, 0, 0, container, { display: 'contents', overflowX: 'hidden' })
    const video = node(0, 0, 600, 400, host)
    Object.assign(video, { assignedSlot: slot })
    expect([...mediaOverlayAncestors(video)]).toEqual([video, slot, container, host])
    expect(visibleMediaRect(video, viewport)).toMatchObject({ left: 120, top: 90, width: 240, height: 180 })
  })

  it('allows a child to override inherited visibility but not ancestor opacity', () => {
    const { node } = fixture()
    const parent = node(0, 0, 400, 300, null, { visibility: 'hidden' })
    expect(visibleMediaRect(node(0, 0, 400, 300, parent), viewport)?.width).toBe(400)
    const transparent = node(0, 0, 400, 300, null, { opacity: '0' })
    expect(visibleMediaRect(node(0, 0, 400, 300, transparent), viewport)).toBeNull()
  })
})

describe('media hover hit testing', () => {
  function fixture() {
    const styles = new WeakMap<object, Record<string, string>>()
    let stack: Element[] = []
    const document = { defaultView: { getComputedStyle: (node: object) => styles.get(node) },
      elementsFromPoint: () => stack }
    const node = (tagName = 'DIV', style = {}, parent: HTMLElement | null = null) => {
      const element = { ownerDocument: document, tagName, parentElement: parent, getRootNode: () => document }
      styles.set(element, { visibility: 'visible', opacity: '1', backgroundColor: 'rgba(0, 0, 0, 0)', backgroundImage: 'none', ...style })
      return element as unknown as HTMLElement
    }
    return { node, document, setStack: (elements: Element[]) => { stack = elements } }
  }

  it('selects the top painted player instead of candidate order or player area', () => {
    const { node, setStack } = fixture()
    const lower = node('VIDEO')
    const upper = node('VIDEO')
    setStack([upper, lower])
    expect(mediaElementAtPoint([lower, upper], { x: 200, y: 100 })).toBe(upper)
    expect(mediaElementAtPoint([lower], { x: 200, y: 100 })).toBeNull()
  })

  it('passes through transparent player controls but stops at a visible modal or image', () => {
    const { node, setStack } = fixture()
    const video = node('VIDEO')
    setStack([node(), video])
    expect(mediaElementAtPoint([video], { x: 200, y: 100 })).toBe(video)
    for (const cover of [node('DIV', { backgroundColor: 'rgb(0, 0, 0)' }), node('IMG'), node('DIV', { backgroundImage: 'url(poster.png)' })]) {
      setStack([cover, video])
      expect(mediaElementAtPoint([video], { x: 200, y: 100 })).toBeNull()
    }
  })

  it('ignores covers hidden by their own visibility or ancestor opacity', () => {
    const { node, setStack } = fixture()
    const video = node('VIDEO')
    const transparentParent = node('DIV', { opacity: '0' })
    for (const cover of [node('DIV', { visibility: 'hidden', backgroundColor: 'rgb(0, 0, 0)' }),
      node('DIV', { backgroundColor: 'rgb(0, 0, 0)' }, transparentParent)]) {
      setStack([cover, video])
      expect(mediaElementAtPoint([video], { x: 200, y: 100 })).toBe(video)
    }
  })

  it('descends into a known closed shadow root while respecting covers outside the host', () => {
    const { node, setStack } = fixture()
    const host = node('DIV', { backgroundColor: 'rgb(0, 0, 0)' })
    const video = node('VIDEO')
    const controls = node()
    const root = { host, elementsFromPoint: () => [controls, video, host] }
    Object.assign(video, { getRootNode: () => root })
    Object.assign(controls, { getRootNode: () => root })
    setStack([host])
    expect(mediaElementAtPoint([video], { x: 200, y: 100 })).toBe(video)
    setStack([node('DIV', { backgroundColor: 'rgb(0, 0, 0)' }), host])
    expect(mediaElementAtPoint([video], { x: 200, y: 100 })).toBeNull()
    expect(mediaElementAtPoint([video], { x: -1, y: -1 })).toBeNull()
  })
})

describe('manual page rescan', () => {
  it('replays MSE evidence and re-reads media elements without opening the panel', () => {
    const content = readFileSync(fileURLToPath(new URL('../entrypoints/content.ts', import.meta.url)), 'utf8')
    const branch = content.slice(content.indexOf("message?.type === 'rescan-media'"), content.indexOf("message?.type === 'rescan-media'") + 1_300)
    expect(branch).toContain('__hls_downloader_replay__')
    expect(branch).toContain("querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video,audio')")
    expect(branch).toContain("performance.getEntriesByType('resource')")
    expect(branch).toContain('Promise.all([...pendingResourceWrites])')
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
