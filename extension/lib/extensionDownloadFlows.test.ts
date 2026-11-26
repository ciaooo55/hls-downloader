import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it, vi } from 'vitest'
import { pageDownloadUrls } from './selectionLinks'
import { mediaElementAtPoint } from './mediaOverlay'
import { bilibiliPlayinfoCandidates, sitePageVideoId } from './siteManifest'
import { isDefaultPopupResource, isSiteHostExcluded, playerPlaybackResources, type MediaResource } from './resources'
import { isDirectDownloadLink } from './directLink'

describe('extension content script flows (page links, shadow players, Bilibili MSE)', () => {
  const content = readFileSync(fileURLToPath(new URL('../entrypoints/content.ts', import.meta.url)), 'utf8')
  const takeoverContent = readFileSync(fileURLToPath(new URL('../entrypoints/takeover.content.ts', import.meta.url)), 'utf8')

  it('keeps message ports open during dormant activation and positions .wrap visibly for collect-page-links', () => {
    // Dormant earlyRuntimeListener must preserve sendResponse and return true
    // for collect-page-links / collect-selection / open-media-panel / download-error
    // so Chrome MV3 does not reject browser.tabs.sendMessage with port-closed errors.
    const earlyBranch = content.slice(
      content.indexOf('const earlyRuntimeListener ='),
      content.indexOf('const srcdocPartitionContextListener ='),
    )
    expect(earlyBranch).toContain('const expectsResponse = kind !== \'captured-resource\'')
    expect(earlyBranch).toContain('if (expectsResponse) return true')

    // .wrap has explicit top-right viewport anchor so selection/page-links panels
    // never open off-screen at the bottom of a tall HTML page.
    expect(content).toContain('.wrap{position:fixed;top:16px;right:16px;')

    // collect-page-links populates resources, renders before opening, updates title, and responds.
    const pageLinksBranch = content.slice(
      content.indexOf("if (message?.type === 'collect-page-links')"),
      content.indexOf("if (message?.type === 'open-media-panel')"),
    )
    expect(pageLinksBranch).toContain('pageDownloadUrls(')
    expect(pageLinksBranch).toContain("setPanelTitle('页面可下载链接')")
    expect(pageLinksBranch).toContain('openFloatingPanel()')
    expect(pageLinksBranch).toContain('sendResponse?.({ ok: true, count: selectionResourceIds.size })')

    // Debian-style directory index page (zero <video> elements) yields downloadable ISO files.
    const harvested = pageDownloadUrls(
      [
        { href: '?C=N;O=D' },
        { href: '/debian-cd/current/amd64/' },
        { href: 'debian-13.8.0-amd64-netinst.iso' },
        { href: 'debian-mac-13.8.0-amd64-netinst.iso' },
      ],
      [],
      'https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/',
    )
    expect(harvested).toEqual([
      'https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-13.8.0-amd64-netinst.iso',
      'https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-mac-13.8.0-amd64-netinst.iso',
    ])
  })

  it('tracks Alt+Click bypass intents on pointerdown and preserves fresh pointerdowns', () => {
    expect(takeoverContent).toContain('altBypass,')
    expect(takeoverContent).toContain("if (event.type === 'click' && lastIntent.key === intentKey")
  })

  it('supports Shadow DOM discovery, window pointermove hover, fullscreen host reparenting, and Bilibili /en/video/ sync', () => {
    // Window-level pointermove captures hover even when player chrome overlays <video>.
    expect(content).toContain("ctx.addEventListener(window, 'pointermove'")
    // Already-playing videos under the pointer bind playback immediately.
    expect(content).toContain('if (!playbackByVideo.has(video) && !video.paused && !video.ended)')
    // Detached-then-connected Shadow DOM custom elements are discovered by mediaObserver.
    expect(content).toContain("const hasShadowCandidate = Boolean(node.shadowRoot) || node.tagName.includes('-')")
    // Fullscreen reparents the shadow host into document.fullscreenElement.
    expect(content).toContain('syncOverlayHostContainer()')
    // Same-video SPA replaceState (/video/BV... -> /en/video/BV...) preserves MSE state.
    expect(content).toContain('if (previousSiteVideoId && previousSiteVideoId === nextSiteVideoId)')

    // End-to-end Bilibili hover + MSE correlation check across /video/ -> /en/video/.
    const cnUrl = 'https://www.bilibili.com/video/BV1xx411c7mD/'
    const enUrl = 'https://www.bilibili.com/en/video/BV1xx411c7mD/'
    expect(sitePageVideoId(cnUrl)).toBe(sitePageVideoId(enUrl))

    const video1080 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30080.m4s?deadline=1900000000'
    const audio192 = 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30280.m4s?deadline=1900000000'
    const [candidate] = bilibiliPlayinfoCandidates(enUrl, {
      data: {
        dash: {
          duration: 120,
          video: [{ id: 80, baseUrl: video1080, width: 1920, height: 1080, bandwidth: 3_000_000, codecs: 'avc1.640032' }],
          audio: [{ id: 30280, baseUrl: audio192, bandwidth: 192_000, codecs: 'mp4a.40.2' }],
        },
      },
    })
    const resource: MediaResource = {
      id: 'bili-1',
      url: candidate.url,
      kind: 'media',
      mimeType: candidate.mimeType,
      pageUrl: enUrl,
      evidence: ['site_player_metadata', 'page_response'],
      seenAt: Date.now(),
      ...candidate.inspection,
    }
    const matched = playerPlaybackResources([resource], {
      sourceUrls: ['blob:https://www.bilibili.com/bpx-mse-1'],
      mseResourceUrls: [video1080],
      startedAt: Date.now(),
    })
    expect(matched).toHaveLength(1)
    expect(matched[0].url).toBe(video1080)

    // Occluded <video> inside a shadow root covered by Bilibili gradient control mask + danmaku canvas.
    const styles = new WeakMap<object, Record<string, string>>()
    let stack: Element[] = []
    const doc = { defaultView: { getComputedStyle: (node: object) => styles.get(node) }, elementsFromPoint: () => stack }
    const makeNode = (tagName: string, style: Record<string, string> = {}) => {
      const el = { ownerDocument: doc, tagName, parentElement: null, getRootNode: () => doc }
      styles.set(el, { visibility: 'visible', opacity: '1', backgroundColor: 'rgba(0, 0, 0, 0)', backgroundImage: 'none', pointerEvents: 'auto', ...style })
      return el as unknown as HTMLElement
    }
    const video = makeNode('VIDEO')
    const shadowHost = makeNode('BPX-PLAYER', { backgroundColor: 'rgb(0, 0, 0)' })
    const topMask = makeNode('DIV', { backgroundImage: 'linear-gradient(to bottom, rgba(0, 0, 0, 0.5), transparent)' })
    const dmCanvas = makeNode('CANVAS', { pointerEvents: 'none' })
    const shadowRoot = { host: shadowHost, elementsFromPoint: () => [topMask, dmCanvas, video, shadowHost] }
    for (const child of [topMask, dmCanvas, video]) Object.assign(child, { getRootNode: () => shadowRoot })
    stack = [shadowHost]
    expect(mediaElementAtPoint([video], { x: 400, y: 200 })).toBe(video)
  })

  it('wires excludedHosts/suppressedSiteHints overlay hiding, DRM guards, visible quick bar, and expanded direct links', () => {
    const popupMain = readFileSync(fileURLToPath(new URL('../entrypoints/popup/main.ts', import.meta.url)), 'utf8')

    // 1. Content script checks excludedHosts & suppressedSiteHints via storage.local + onChanged and hides video buttons.
    expect(content).toContain("browser.storage.local.get(['excludedHosts', 'suppressedSiteHints', 'tvboxConfigured'])")
    expect(content).toContain('const siteExcluded = isCurrentSiteExcluded()')
    expect(content).toContain('const choices = allChoices.filter(resource => !resource.drmProtected)')
    expect(isSiteHostExcluded('sub.example.com', ['example.com'])).toBe(true)
    expect(isSiteHostExcluded('other.org', ['example.com'])).toBe(false)

    // 2. Popup uses an always-visible quick-settings-bar (not closed <details>), keeps non-colliding aria-labels,
    //    preserves 本站提示：已显示/已隐藏 aria-label contract, and hides TVBox when tvboxConfigured === false.
    expect(popupMain).toContain("el('div', 'quick-settings-bar')")
    expect(popupMain).not.toContain("el('details', 'quick-settings-drawer')")
    expect(popupMain).toContain('[excludeBtn, \'本站提示与接管\', \'在此网站隐藏悬浮下载按钮，并暂停自动接管下载。\']')
    expect(popupMain).toContain('`本站提示：${excludeBtn.textContent}`')
    expect(popupMain).toContain("activePanelIndex === 2 ? `自动接管：${enableStateLabel}` : `全局自动接管：${enableStateLabel}`")
    expect(popupMain).toContain('DRM 加密保护 · 不支持直接下载')
    expect(popupMain).toContain('tvboxConfigured === false')

    // 3. takeover.content.ts falls back cleanly when runtime.sendMessage rejects instead of swallowing the click.
    expect(takeoverContent).toContain('void fallback()')

    // 4. Expanded directLink extensions (.safetensors, .gguf, .whl, .mp4, .pdf, .zst, .appimage).
    for (const ext of ['model.safetensors', 'weights.gguf', 'pkg-1.0-py3-none-any.whl', 'video.mp4', 'paper.pdf', 'archive.tar.zst', 'tool.AppImage']) {
      expect(isDirectDownloadLink(`https://cdn.test/files/${ext}`, 'https://site.test/', false)).toBe(true)
    }

    // 5. Popup default view includes high-confidence inspected manifests, Bilibili dual-track, and audio/current_src media.
    expect(isDefaultPopupResource({ id: '1', url: 'https://cdn.test/master.m3u8', kind: 'hls', inspected: true, seenAt: 1 })).toBe(true)
    expect(isDefaultPopupResource({ id: '2', url: 'https://cdn.test/song.mp3', kind: 'media', mimeType: 'audio/mpeg', seenAt: 1 })).toBe(true)
    expect(isDefaultPopupResource({ id: '3', url: 'https://cdn.test/paused.mp4', kind: 'media', evidence: ['current_src'], seenAt: 1 })).toBe(true)
  })

  it('guards content overlay 2.5s auto-reset timers by generation and prioritizes active download-states in popup (G8-7, G8-1)', async () => {
    vi.useFakeTimers()
    vi.stubGlobal('defineContentScript', (definition: unknown) => definition)
    try {
      const { createSendResetTracker } = await import('../entrypoints/content')
      const tracker = createSendResetTracker()
      let currentState = 'idle'

      // Attempt 1 ends in 'canceled' and schedules a 2.5s clear timer.
      const gen1 = tracker.nextGeneration('hls:https://cdn.test/master.m3u8:download')
      currentState = 'canceled'
      tracker.scheduleReset('hls:https://cdn.test/master.m3u8:download', gen1, 2_500, () => {
        currentState = 'idle'
      })

      // 1.0s later, user clicks retry -> gen2 starts ('sending' -> 'confirming').
      await vi.advanceTimersByTimeAsync(1_000)
      const gen2 = tracker.nextGeneration('hls:https://cdn.test/master.m3u8:download')
      expect(tracker.isCurrent('hls:https://cdn.test/master.m3u8:download', gen1)).toBe(false)
      expect(tracker.isCurrent('hls:https://cdn.test/master.m3u8:download', gen2)).toBe(true)
      currentState = 'confirming'

      // At t = 2.5s (1.5s later), gen1's old timer MUST NOT wipe 'confirming'!
      await vi.advanceTimersByTimeAsync(2_000)
      expect(currentState).toBe('confirming')

      // Even a late scheduleReset with stale gen1 is ignored.
      tracker.scheduleReset('hls:https://cdn.test/master.m3u8:download', gen1, 2_500, () => {
        currentState = 'idle'
      })
      await vi.advanceTimersByTimeAsync(3_000)
      expect(currentState).toBe('confirming')

      // When gen2 settles into 'queued', its own 2.5s timer clears cleanly.
      currentState = 'queued'
      tracker.scheduleReset('hls:https://cdn.test/master.m3u8:download', gen2, 2_500, () => {
        currentState = 'idle'
      })
      await vi.advanceTimersByTimeAsync(2_499)
      expect(currentState).toBe('queued')
      await vi.advanceTimersByTimeAsync(1)
      expect(currentState).toBe('idle')

      // Verify content.ts wires sendResetTracker across sendResource, pushToTv, and castResource,
      // and popup/main.ts resolves best/newest handoff state per key instead of locking onto stale 'canceled'.
      expect(content).toContain('const generation = sendResetTracker.nextGeneration(key)')
      expect(content).toContain('sendResetTracker.scheduleReset(key, generation, 2_500,')
      const popupMain = readFileSync(fileURLToPath(new URL('../entrypoints/popup/main.ts', import.meta.url)), 'utf8')
      expect(popupMain).toContain('const resolvedByKey = new Map<')
      expect(popupMain).toContain('nextPriority > prevPriority')
    } finally {
      vi.unstubAllGlobals()
      vi.useRealTimers()
    }
  })
})
