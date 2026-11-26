import { browser } from 'wxt/browser'
import { HOVER_DISMISS_DELAY_MS, clampOverlayPosition, mediaElementAtPoint, mediaOverlayAncestors, overlayActionFallback, overlayQualityOptions, overlayResourceDetails, overlaySendKey, safeResourceLocation, shouldKeepHoverOpen, shouldShowMediaOverlay, visibleMediaRect, type OverlayAction, type OverlayQualityOption, type OverlayRect } from '../lib/mediaOverlay'
import { boundedConfidence, classifyPlaybackSource, classifyResource, compactResources, isGenericMediaName, isPlayableMediaResource, isSameDocumentPlaybackFallback, isSiteHostExcluded, mergeResources, normalizeHost, playerPlaybackResources, resourceFingerprint, resourceId, resourceMatchesPlaybackSource, resourceRank, selectMediaVariant, visiblePlaybackResources, type MediaResource, type PlaybackContext } from '../lib/resources'
import { patchKeyedChildren, setAttrIfChanged, setTextIfChanged, syncSelectOptions } from '../lib/domPatch'
import { resourceQuality } from '../lib/hlsManifest'
import { isMseMediaSliceUrl } from '../lib/manifestSniff'
import { THEME_BASE_CSS, THEME_STORAGE_KEY, THEME_TOKENS_CSS, applyTheme, normalizeThemePreference } from '../lib/theme'
import { withDeadline } from '../lib/asyncDeadline'
import { waitForDownloadConfirmation } from '../lib/downloadConfirmation'
import { SEND_LABELS, SEND_LOCKED, SEND_TITLES, canForceResend, sendClickAction, sendStateFromHandoff, type SendState } from '../lib/downloadSendState'
import { mediaPushTerminalResult } from '../lib/mediaPush'
import { pageDownloadUrls, selectedDownloadUrls } from '../lib/selectionLinks'
import { HARVEST_CATEGORY_CHIPS, countHarvestCategories, filterHarvestResources, type HarvestLinkCategory } from '../lib/harvestLinks'
import { formatBitrate, formatBytes, formatDuration } from '../lib/format'
import { installSubtitleContent } from '../lib/subtitleContent'
import { isBilibiliAudioTrackMseUrl, isBilibiliFullTrackMseUrl, sameSiteVideoIdentity, sitePageVideoId } from '../lib/siteManifest'

async function runtimeMessage(message: Record<string, unknown>, retries = 1): Promise<any> {
  let lastError: unknown
  for (let attempt = 0; attempt <= retries; attempt += 1) {
    try {
      return await browser.runtime.sendMessage(message)
    } catch (error) {
      lastError = error
      if (attempt < retries) await new Promise(resolve => setTimeout(resolve, 180))
    }
  }
  const detail = lastError instanceof Error ? lastError.message : String(lastError || '')
  if (/receiving end does not exist|extension context invalidated/i.test(detail)) {
    throw new Error('扩展已更新或后台未连接，请刷新当前网页后重试')
  }
  throw lastError
}

// Firefox 不允许 MAIN/隔离脚本直接读取对方的对象，桥接数据用 JSON 字符串传递。
function bridgeDetail(event: Event): Record<string, any> {
  try {
    const detail = (event as CustomEvent).detail
    if (typeof detail !== 'string' || detail.length > 64 * 1024) return {}
    const parsed = JSON.parse(detail)
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {}
  } catch { return {} }
}

export interface SendResetScheduler {
  setTimeout(callback: () => void, delayMs: number): ReturnType<typeof setTimeout> | number
  clearTimeout?(handle: any): void
}

export function createSendResetTracker(scheduler: SendResetScheduler = globalThis) {
  const generations = new Map<string, number>()
  const timers = new Map<string, ReturnType<typeof setTimeout> | number>()
  const clearTimer = (key: string) => {
    const existing = timers.get(key)
    if (existing !== undefined) {
      ;(scheduler.clearTimeout ?? clearTimeout)(existing as ReturnType<typeof setTimeout>)
      timers.delete(key)
    }
  }
  return {
    nextGeneration(key: string): number {
      clearTimer(key)
      const next = (generations.get(key) || 0) + 1
      generations.set(key, next)
      return next
    },
    isCurrent(key: string, generation: number): boolean {
      return generations.get(key) === generation
    },
    scheduleReset(key: string, generation: number, delayMs: number, reset: () => void): void {
      if (generations.get(key) !== generation) return
      clearTimer(key)
      const handle = scheduler.setTimeout(() => {
        if (timers.get(key) === handle) timers.delete(key)
        if (generations.get(key) !== generation) return
        reset()
      }, delayMs)
      timers.set(key, handle)
    },
    clear(key: string): void {
      clearTimer(key)
    },
  }
}

export default defineContentScript({
  matches: ['<all_urls>'],
  allFrames: true,
  matchAboutBlank: true,
  matchOriginAsFallback: true,
  // 媒体或其他子资源迟迟未完成时，网页操作入口仍应在 DOM 就绪后出现。
  runAt: 'document_end',
  // All overlay/theme CSS is constructed below inside the isolated root. `ui`
  // makes WXT fetch a generated content.css even when no CSS bundle exists,
  // producing one denied chrome-extension request on every page.
  cssInjectionMode: 'manual',
  async main(ctx) {
    const pendingResourceWrites = new Set<Promise<unknown>>()
    const eventVideo = (event: Event): HTMLVideoElement | null => {
      const path = typeof event.composedPath === 'function' ? event.composedPath() : []
      return (path.find(value => value instanceof HTMLVideoElement)
        || (event.target instanceof HTMLVideoElement ? event.target : null)) as HTMLVideoElement | null
    }
    const eventAudio = (event: Event): HTMLAudioElement | null => {
      const path = typeof event.composedPath === 'function' ? event.composedPath() : []
      return (path.find(value => value instanceof HTMLAudioElement)
        || (event.target instanceof HTMLAudioElement ? event.target : null)) as HTMLAudioElement | null
    }
    let activateContent: (() => void) | null = null
    let activationRequested = false
    const activation = new Promise<void>(resolve => { activateContent = resolve })
    const requestActivation = () => {
      if (activationRequested || ctx.isInvalid) return
      activationRequested = true
      activateContent?.()
    }
    // The shadow UI is mounted asynchronously.  A fast player can emit its
    // first play/playing event while that work is still in progress (Firefox
    // exposes this race more readily than Chromium).  Buffer those events so
    // the per-video entry is never lost, and expose the ready marker only
    // after the real listeners are installed.
    document.documentElement.setAttribute('data-hls-downloader-extension', 'loading')
    const pendingPlaybackVideos = new Set<HTMLVideoElement>()
    const pendingPlaybackAudios = new Set<HTMLAudioElement>()
    const rememberPendingPlayback = (event: Event) => {
      const video = eventVideo(event)
      if (video) {
        pendingPlaybackVideos.add(video)
        requestActivation()
        return
      }
      const audio = eventAudio(event)
      if (audio) {
        pendingPlaybackAudios.add(audio)
        requestActivation()
      }
    }
    ctx.addEventListener(document, 'play', rememberPendingPlayback, { capture: true })
    ctx.addEventListener(document, 'playing', rememberPendingPlayback, { capture: true })
    const resources = new Map<string, MediaResource>()
    let playbackByVideo = new WeakMap<HTMLVideoElement, PlaybackContext>()
    const discoverSubtitlePlayers = installSubtitleContent(ctx, media => {
      const candidates = playerPlaybackResources([...resources.values()], media instanceof HTMLVideoElement
        ? playbackByVideo.get(media) || null : { sourceUrls: [media.currentSrc], startedAt: 0 }, 8)
      const exact = candidates.filter(resource => resourceMatchesPlaybackSource(resource, media.currentSrc))
      const choices = exact.length ? exact : candidates
      return choices.length === 1 ? choices[0] : undefined
    })
    // The UI mount below is asynchronous. Capture bridge/webRequest messages
    // during that window instead of losing the first manifest from a fast MSE
    // player (which otherwise leaves the overlay in “正在识别” forever).
    const samePageOrVideo = (sourceUrl: string, targetUrl: string): boolean => {
      const left = pageKey(sourceUrl)
      const right = pageKey(targetUrl)
      return left === right || sameSiteVideoIdentity(left, right)
    }
    const earlyResourceEvents: Array<{ url: string; mimeType?: string; audioUrl?: string; inspection?: Partial<MediaResource>; siteVideoId?: string; sourcePageUrl?: string; responseIdentity?: Record<string, string> }> = []
    const earlyMseEvents: Array<{ blobUrl: string; mediaUrl: string; purpose?: 'mse' | 'download'; sourcePageUrl?: string }> = []
    const earlyRuntimeMessages: Array<{ message: any; sendResponse?: (value?: unknown) => void }> = []
    let contentReady = false
    const earlyResourceListener = (event: Event) => {
      if (contentReady) return
      const detail = bridgeDetail(event)
      if (detail.sourcePageUrl && !samePageOrVideo(detail.sourcePageUrl, location.href)) return
      if (typeof detail.url === 'string') {
        earlyResourceEvents.push({
          url: detail.url,
          mimeType: detail.mimeType,
          audioUrl: typeof detail.audioUrl === 'string' ? detail.audioUrl : undefined,
          inspection: detail.inspection,
          siteVideoId: detail.siteVideoId,
          sourcePageUrl: detail.sourcePageUrl,
          responseIdentity: detail.responseIdentity,
        })
        if (classifyResource(detail.url, detail.mimeType) || detail.siteVideoId) requestActivation()
      }
      if (earlyResourceEvents.length > 200) earlyResourceEvents.splice(0, earlyResourceEvents.length - 200)
    }
    const earlyMseListener = (event: Event) => {
      if (contentReady) return
      const detail = bridgeDetail(event)
      if (detail.sourcePageUrl && !samePageOrVideo(detail.sourcePageUrl, location.href)) return
      if (typeof detail.blobUrl === 'string' && typeof detail.mediaUrl === 'string') {
        earlyMseEvents.push({
          blobUrl: detail.blobUrl,
          mediaUrl: detail.mediaUrl,
          purpose: detail.purpose === 'download' ? 'download' : 'mse',
          sourcePageUrl: detail.sourcePageUrl,
        })
        if (earlyMseEvents.length > 200) earlyMseEvents.splice(0, earlyMseEvents.length - 200)
        if (detail.purpose === 'download') {
          // Do not wait for the media UI. downloads.onCreated can fire while
          // overlay activation is still pending, and an unmatched blob: item
          // is left entirely in the browser.
          void runtimeMessage({
            type: 'blob-source',
            blobUrl: detail.blobUrl,
            sourceUrl: detail.mediaUrl,
            pageUrl: location.href,
          }).catch(() => undefined)
        }
        requestActivation()
      }
    }
    const earlyRuntimeListener = (message: any, _sender?: unknown, sendResponse?: (value?: unknown) => void) => {
      const kind = String(message?.type || '')
      if (contentReady || !['captured-resource', 'collect-selection', 'collect-page-links', 'open-media-panel', 'download-error', 'rescan-media'].includes(kind)) return
      if (kind === 'captured-resource' && !message.resource?.url) return
      const expectsResponse = kind !== 'captured-resource'
      earlyRuntimeMessages.push({ message, sendResponse: expectsResponse ? sendResponse : undefined })
      if (earlyRuntimeMessages.length > 100) earlyRuntimeMessages.splice(0, earlyRuntimeMessages.length - 100)
      requestActivation()
      if (expectsResponse) return true
    }
    const srcdocPartitionContextListener = (message: any, _sender: any, sendResponse?: (response?: unknown) => void) => {
      if (message?.type !== 'srcdoc-partition-context') return
      const frameLocation = location as Location & { ancestorOrigins?: DOMStringList }
      sendResponse?.({
        href: location.href,
        origin: String((globalThis as any).origin || ''),
        ancestorOrigins: Array.from(frameLocation.ancestorOrigins || []),
      })
    }
    ctx.addEventListener(window, '__hls_downloader_resource__', earlyResourceListener)
    ctx.addEventListener(window, '__hls_downloader_mse__', earlyMseListener)
    browser.runtime.onMessage.addListener(earlyRuntimeListener)
    browser.runtime.onMessage.addListener(srcdocPartitionContextListener)
    ctx.onInvalidated(() => browser.runtime.onMessage.removeListener(srcdocPartitionContextListener))
    ctx.onInvalidated(() => browser.runtime.onMessage.removeListener(earlyRuntimeListener))
    const activationRoots = new WeakSet<Document | ShadowRoot>()
    const activationMediaRoots: Array<Document | ShadowRoot> = []
    const activationObservers: MutationObserver[] = []
    const discoverActivationShadowRoots = (node: ParentNode | Element) => {
      // Do not build a NodeList for every element in a busy live/chat page.
      // This early race-closing scan is best effort; play events and the
      // document-start attachShadow hook still discover every later player.
      const inspect = (element: Element) => {
        if (element.shadowRoot) observeActivationRoot(element.shadowRoot)
      }
      if (node instanceof Element) inspect(node)
      const walker = document.createTreeWalker(node, NodeFilter.SHOW_ELEMENT)
      let visited = 0
      let current = walker.nextNode() as Element | null
      while (current && visited < 4096) {
        inspect(current)
        visited += 1
        current = walker.nextNode() as Element | null
      }
    }
    const observeActivationRoot = (root: Document | ShadowRoot) => {
      if (activationRoots.has(root)) return
      activationRoots.add(root)
      activationMediaRoots.push(root)
      discoverSubtitlePlayers(root)
      ctx.addEventListener(root, 'play', rememberPendingPlayback, { capture: true })
      ctx.addEventListener(root, 'playing', rememberPendingPlayback, { capture: true })
      const observer = new MutationObserver(mutations => {
        for (const mutation of mutations) {
          if (mutation.type === 'attributes') {
            if (mutation.target instanceof Element && mutation.target.matches('video,audio,source')) {
              requestActivation()
              return
            }
            continue
          }
          for (const node of mutation.addedNodes) {
            if (!(node instanceof Element)) continue
            if (node.shadowRoot) observeActivationRoot(node.shadowRoot)
            else if (node.tagName.includes('-')) discoverActivationShadowRoots(node)
            if (node.matches('video,audio,source[src]') || node.querySelector('video,audio,source[src]')) {
              requestActivation()
              return
            }
          }
        }
      })
      observer.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] })
      activationObservers.push(observer)
      ctx.onInvalidated(() => observer.disconnect())
      if (root.querySelector('video,audio,source[src]')) {
        requestActivation()
      }
      // 启动前创建的嵌套 ShadowRoot 不会重放 attachShadow/play 事件，需逐层发现。
      if (root instanceof ShadowRoot) discoverActivationShadowRoots(root)
    }
    const earlyShadowListener = (event: Event) => {
      const host = event.composedPath().find(value => value instanceof Element) as Element | undefined
      if (host?.shadowRoot) observeActivationRoot(host.shadowRoot)
    }
    ctx.addEventListener(window, '__hls_downloader_shadow__', earlyShadowListener, { capture: true })
    observeActivationRoot(document)
    // The page may create an open ShadowRoot from an inline document-start
    // script before this isolated-world bootstrap is scheduled. One bounded
    // initial scan closes that race without enabling the full DOM observer/UI.
    discoverActivationShadowRoots(document)
    // The bootstrap is ready for download-intent and background messages even
    // though the expensive media UI remains dormant until concrete evidence.
    document.documentElement.setAttribute('data-hls-downloader-extension', '1')
    await activation
    if (ctx.isInvalid) return
    activationObservers.forEach(observer => observer.disconnect())
    activationObservers.length = 0
    // Rendering is intentionally frequent on live pages. Keep handoff state
    // outside the DOM so a network event cannot replace an in-flight button.
    const resourceSendStates = new Map<string, { label: string, disabled: boolean, error?: string, state?: SendState, url?: string, updatedAt?: number }>()
    const preferredVariants = new Map<string, { variantId?: string, height: number, bandwidth: number }>()
    let renderedListSignature = ''
    let renderedVideoSignature = ''
    let activePlayback: PlaybackContext | null = null
    let activeVideo: HTMLVideoElement | null = null
    let selectionMode = false
    let selectionResourceIds: Set<string> | null = null
    let selectionEmptyMessage = '选中的内容里没有可下载链接'
    let harvestMode = false
    let harvestCategory: HarvestLinkCategory = 'all'
    let harvestQuery = ''
    let harvestBatchBusy = false
    const harvestCheckedIds = new Set<string>()
    // A page can contain several players.  Keep playback evidence per element
    // instead of treating the latest network manifest as belonging to every
    // video in the document.
    let hoveredVideo: HTMLVideoElement | null = null
    let playerDismissTimer: ReturnType<typeof setTimeout> | undefined
    const knownVideos = new Set<HTMLVideoElement>()
    const knownAudios = new Set<HTMLAudioElement>()
    const observedVideoSizes = new WeakSet<HTMLVideoElement>()
    let videoResizeObserver: ResizeObserver | null = null
    let videoIntersectionObserver: IntersectionObserver | null = null
    let excludedSiteHosts: string[] = []
    let suppressedHintHosts: string[] = []
    let tvboxConfigured: boolean | undefined
    const normalizeHostList = (value: unknown): string[] => Array.isArray(value)
      ? value.map(item => normalizeHost(String(item || ''))).filter(Boolean)
      : []
    const currentSiteHosts = (): string[] => {
      const hosts = new Set<string>()
      const own = normalizeHost(location.hostname)
      if (own) hosts.add(own)
      const frameLocation = location as Location & { ancestorOrigins?: DOMStringList }
      for (const origin of Array.from(frameLocation.ancestorOrigins || [])) {
        const ancestorHost = normalizeHost(origin)
        if (ancestorHost) hosts.add(ancestorHost)
      }
      if (window !== window.top && document.referrer) {
        const refHost = normalizeHost(document.referrer)
        if (refHost) hosts.add(refHost)
      }
      return [...hosts]
    }
    const isCurrentSiteExcluded = (): boolean => {
      if (!excludedSiteHosts.length && !suppressedHintHosts.length) return false
      const combined = [...excludedSiteHosts, ...suppressedHintHosts]
      return currentSiteHosts().some(siteHost => isSiteHostExcluded(siteHost, combined))
    }
    const mediaOwnerIds = new WeakMap<HTMLMediaElement, string>()
    let nextMediaOwnerId = 1
    const mediaOwner = (media: HTMLMediaElement) => {
      const existing = mediaOwnerIds.get(media)
      if (existing) return existing
      const owner = `media-element:${nextMediaOwnerId++}`
      mediaOwnerIds.set(media, owner)
      return owner
    }
    const currentVideos = () => {
      for (const video of knownVideos) {
        if (!video.isConnected) {
          videoResizeObserver?.unobserve(video)
          videoIntersectionObserver?.unobserve(video)
          observedVideoSizes.delete(video)
          knownVideos.delete(video)
        }
      }
      return [...knownVideos]
    }
    const mseEvidenceByBlob = new Map<string, { urls: Set<string>; seenAt: number }>()
    const replaceResources = (values: MediaResource[]) => {
      resources.clear()
      for (const value of values) resources.set(resourceFingerprint(value), value)
    }
    const resourcePresentationSignature = (items = [...resources.values()], includePlaybackEvidence = true) => items
      .map(resource => [
        resourceFingerprint(resource), resource.url, resource.audioUrl || '', resource.title || '', resource.filename || '',
        resource.kind, resource.mimeType || '', resource.size || 0, resource.estimatedSize || 0,
        resource.width || 0, resource.height || 0, resource.bandwidth || 0, resource.duration || 0,
        resource.isLive === true ? 1 : resource.isLive === false ? 0 : '', resource.inspected ? 1 : 0, resource.drmProtected ? 1 : 0,
        (resource.variants || []).map(variant => `${variant.url}:${variant.audioUrl || ''}:${variant.width || 0}:${variant.height || 0}:${variant.bandwidth || 0}`).join(','),
        includePlaybackEvidence ? (resource.playbackUrls || []).join(',') : '',
        includePlaybackEvidence ? (resource.playbackPatterns || []).join(',') : '',
        includePlaybackEvidence ? (resource.playbackUrlHashes || []).join(',') : '',
      ].join('|'))
      .sort()
      .join('\n')
    const addResource = (resource: MediaResource) => {
      const before = resourcePresentationSignature()
      replaceResources(mergeResources([...resources.values()], resource, 240))
      return before !== resourcePresentationSignature()
    }
    const pageMediaTitle = () => {
      const metadata = [
        document.querySelector<HTMLMetaElement>('meta[property="og:title"]')?.content,
        document.querySelector<HTMLMetaElement>('meta[name="twitter:title"]')?.content,
        document.querySelector<HTMLElement>('[itemprop="name"]')?.getAttribute('content'),
        document.title,
        document.querySelector<HTMLElement>('h1')?.innerText,
      ]
      return metadata.find(value => value?.trim())?.trim().replace(/^\(\d+\)\s*/, '') || ''
    }
    const ui = await createShadowRootUi(ctx, {
      name: 'hls-downloader-media-panel', position: 'inline', anchor: () => document.body || document.documentElement,
      onMount(container) {
        const element = <K extends keyof HTMLElementTagNameMap>(tag: K, className = '', text = '') => {
          const node = document.createElement(tag)
          if (className) node.className = className
          if (text) node.textContent = text
          return node
        }
        const root = document.createElement('div')
        const iconUrl = browser.runtime.getURL('/icon-32.png')
        const style = element('style')
        style.textContent = `
          :host{all:initial}*{box-sizing:border-box}[hidden]{display:none!important}button{font:13px "Segoe UI Variable Text","Segoe UI",system-ui,sans-serif;letter-spacing:0}
          ${THEME_TOKENS_CSS}
          ${THEME_BASE_CSS}
          .panel-backdrop{position:fixed;inset:0;z-index:calc(var(--z-extension-overlay) - 1);background:rgba(9,12,18,.38);backdrop-filter:blur(2px);opacity:0;visibility:hidden;pointer-events:none;transition:opacity .16s ease,visibility 0s linear .16s}.panel-backdrop.open{opacity:1;visibility:visible;pointer-events:auto;transition-delay:0s}
          .wrap{position:fixed;top:16px;right:16px;z-index:var(--z-extension-overlay);color:var(--text);pointer-events:none;visibility:hidden;transition:visibility 0s linear .18s}.wrap.open{pointer-events:auto;visibility:visible;transition-delay:0s}
          .panel{width:min(352px,calc(100vw - 20px));max-height:min(528px,calc(100vh - 20px));background:var(--glass-strong);backdrop-filter:blur(16px) saturate(160%);border:1px solid var(--overlay-border);border-radius:12px;box-shadow:var(--shadow-lg),inset 0 1px 0 var(--glass-rim);overflow:hidden;opacity:0;transform:translateY(6px) scale(.985);transform-origin:top right;pointer-events:none;transition:opacity .18s cubic-bezier(.22,1,.36,1),transform .18s cubic-bezier(.22,1,.36,1)}.wrap.open .panel{opacity:1;transform:translateY(0) scale(1);pointer-events:auto}.wrap.harvest-mode .panel{display:flex;flex-direction:column;width:min(436px,calc(100vw - 20px));max-height:min(82vh,640px)}
          header{display:flex;align-items:center;justify-content:space-between;padding:9px 11px;border-bottom:1px solid var(--border);background:color-mix(in srgb,var(--surface-2) 86%,transparent);color:var(--text);font:650 12.5px "Segoe UI Variable Text","Segoe UI",system-ui;cursor:grab;touch-action:none}.title{display:flex;align-items:center;gap:7px}.title img{width:18px;height:18px;border-radius:5px;box-shadow:var(--shadow-sm)}.head-actions{display:flex;align-items:center;gap:5px}
          .pin,.close{height:27px;border:1px solid var(--border);border-radius:var(--radius);background:var(--surface);color:var(--text);cursor:pointer}.pin{padding:0 9px;font:600 11px "Segoe UI Variable Text","Segoe UI",system-ui}.pin.active{border-color:color-mix(in srgb,var(--green) 40%,transparent);background:color-mix(in srgb,var(--green) 18%,var(--surface-3));color:var(--green-ink)}.close{display:grid;place-items:center;width:27px;font:600 17px/1 system-ui}.pin:hover,.close:hover{border-color:color-mix(in srgb,var(--primary) 42%,var(--border));background:color-mix(in srgb,var(--primary) 14%,var(--surface-3))}.list{max-height:calc(min(528px,calc(100vh - 20px)) - 80px);overflow-y:auto;overscroll-behavior:contain}.wrap.harvest-mode .list{max-height:min(62vh,460px)}.list::-webkit-scrollbar{width:7px}.list::-webkit-scrollbar-thumb{border:2px solid transparent;border-radius:999px;background:color-mix(in srgb,var(--faint) 55%,transparent);background-clip:content-box}.list::-webkit-scrollbar-track{background:transparent}
          .harvest-toolbar{display:grid;gap:7px;padding:9px 11px;border-bottom:1px solid var(--border);background:var(--surface)}.harvest-search-row{display:flex;align-items:center;gap:6px}.harvest-search{flex:1;min-width:0;height:29px;padding:0 9px;border:1px solid var(--control-border);border-radius:var(--radius);background:var(--surface-2);color:var(--text);font:12px "Segoe UI Variable Text","Segoe UI",system-ui,sans-serif;transition:border-color .14s ease,background-color .14s ease}.harvest-search::placeholder{color:var(--faint)}.harvest-search:hover{border-color:var(--primary);background:var(--surface)}.harvest-search:focus-visible{outline:2px solid var(--focus-ring);outline-offset:1px;border-color:var(--primary)}.harvest-chips{display:flex;flex-wrap:wrap;gap:4px}.harvest-chip{display:inline-flex;align-items:center;gap:4px;height:24px;padding:0 8px;border:1px solid var(--border);border-radius:var(--radius-pill);background:var(--surface-2);color:var(--muted);cursor:pointer;font:600 11px/1 "Segoe UI Variable Text","Segoe UI",system-ui,sans-serif;transition:background-color .14s ease,color .14s ease,border-color .14s ease,transform .12s ease}.harvest-chip:hover{border-color:color-mix(in srgb,var(--primary) 42%,var(--border));background:var(--surface-3);color:var(--text)}.harvest-chip:active{transform:scale(.96)}.harvest-chip[aria-pressed=true]{border-color:color-mix(in srgb,var(--primary) 48%,var(--border));background:color-mix(in srgb,var(--primary) 16%,var(--surface-2));color:var(--primary-ink)}.harvest-chip b{font-weight:700;font-variant-numeric:tabular-nums}.harvest-batch-bar{display:flex;align-items:center;justify-content:space-between;gap:8px}.harvest-select-all{display:inline-flex;align-items:center;gap:6px;height:28px;padding:0 9px;border:1px solid var(--control-border);border-radius:var(--radius);background:var(--surface-2);color:var(--text);cursor:pointer;font:600 11.5px/1 "Segoe UI Variable Text","Segoe UI",system-ui,sans-serif;transition:background-color .14s ease,border-color .14s ease,transform .12s ease}.harvest-select-all:hover{border-color:var(--primary);background:var(--surface-3)}.harvest-select-all:active{transform:scale(.97)}.download.harvest-batch-btn{flex:none;height:28px;padding:0 12px;font-variant-numeric:tabular-nums}
          .item{padding:10px 11px;border-bottom:1px solid var(--border);transition:background-color .14s ease}.item:last-child{border-bottom:0}.item:hover{background:color-mix(in srgb,var(--surface-2) 88%,transparent)}.item-head{display:flex;align-items:flex-start;gap:8px}.item-check{width:15px;height:15px;margin:2px 0 0;flex:none;accent-color:var(--primary);cursor:pointer}.item-head .meta{flex:1;min-width:0}.meta{min-width:0}.name{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;font:600 12.5px/1.38 "Segoe UI Variable Text","Segoe UI",system-ui;overflow-wrap:anywhere;color:var(--text)}.kind{overflow:hidden;color:var(--muted);font:11px/1.35 system-ui;font-variant-numeric:tabular-nums;margin-top:3px;text-overflow:ellipsis;white-space:nowrap}.resource-url{display:block;margin-top:5px;padding:3px 6px;border-radius:5px;background:var(--surface-2);color:var(--faint);font:10px/1.4 ui-monospace,SFMono-Regular,Consolas,monospace;overflow-wrap:anywhere;user-select:text}.quality-select{display:block;width:min(196px,100%);height:29px;margin-top:7px;font-size:11.5px}.item-actions{display:flex;gap:5px;margin-top:8px}.download{min-width:0;flex:1;height:30px;border:1px solid transparent;border-radius:var(--radius);background:var(--primary);color:var(--on-primary);padding:4px 8px;cursor:pointer;font-weight:600;font-size:11.5px;box-shadow:inset 0 1px 0 rgba(255,255,255,.2),var(--shadow-sm)}.download:hover{background:var(--primary-hover)}.download[disabled]{cursor:default;opacity:.6}.download.resend{flex:none;padding:0 9px;border-color:color-mix(in srgb,var(--primary) 36%,var(--border));background:var(--surface-2);color:var(--primary-ink);box-shadow:none}.download.resend:hover:not([disabled]){border-color:transparent;background:var(--primary);color:var(--on-primary)}.download.push-tv,.download.cast{border-color:var(--border);background:var(--surface-2);color:var(--text);box-shadow:none}.download.push-tv:hover{border-color:transparent;background:var(--purple);color:var(--on-primary)}.download.cast:hover{border-color:transparent;background:var(--green);color:var(--on-primary)}.result{padding:8px 11px;border-bottom:1px solid var(--border);background:color-mix(in srgb,var(--green) 14%,var(--surface));color:var(--green-ink);font:600 11px/1.4 system-ui}.result.error{background:color-mix(in srgb,var(--red) 12%,var(--surface));color:var(--red-ink)}.result.info{background:var(--surface-2);color:var(--muted)}
          .video-buttons{position:fixed;inset:0;z-index:var(--z-extension-video);pointer-events:none}.video-action-group{position:fixed;display:flex;align-items:center;gap:5px;pointer-events:none;opacity:0;transform:translateY(4px) scale(.96);transition:opacity .18s cubic-bezier(.22,1,.36,1),transform .18s cubic-bezier(.22,1,.36,1)}.video-action-group.visible{opacity:1;transform:translateY(0) scale(1);pointer-events:auto}.video-action-group.dragging{transition:none}.video-download{position:relative;display:flex;align-items:center;gap:7px;height:34px;padding:0 13px 0 10px;border:1px solid color-mix(in srgb,var(--primary) 65%,rgba(255,255,255,.28));border-radius:var(--radius-pill);background:color-mix(in srgb,var(--primary) 92%,transparent);backdrop-filter:blur(12px) saturate(160%);color:var(--on-primary);box-shadow:inset 0 1px 0 rgba(255,255,255,.26),0 4px 14px var(--shadow),0 0 0 1px rgba(0,0,0,.18);cursor:pointer;touch-action:none;user-select:none;-webkit-user-select:none;font:650 12px "Segoe UI Variable Text","Segoe UI",system-ui;font-variant-numeric:tabular-nums;transition:background-color .16s ease,transform .16s ease,box-shadow .16s ease}.video-action-group.dragging .video-download{cursor:grabbing}.video-download:hover{background:var(--primary-hover)}.video-download.identifying{border-color:var(--overlay-border);background:var(--glass-strong);color:var(--muted);box-shadow:var(--shadow-md)}.video-download.identifying:hover{background:var(--glass-strong)}.video-download img{width:18px;height:18px;border-radius:50%;box-shadow:0 1px 2px rgba(0,0,0,.22)}.video-download b{display:inline-grid;place-items:center;min-width:19px;height:19px;padding:0 5px;border-radius:var(--radius-pill);background:var(--on-primary);color:var(--primary);font:700 10px "Segoe UI Variable Text","Segoe UI",system-ui;font-variant-numeric:tabular-nums}.video-more{display:grid;width:34px;height:34px;place-items:center;padding:0 0 1px;border:1px solid var(--overlay-border);border-radius:50%;background:var(--glass-strong);backdrop-filter:blur(12px) saturate(160%);color:var(--primary);box-shadow:0 4px 14px var(--shadow),inset 0 1px 0 var(--glass-rim);cursor:pointer;font:700 17px/1 system-ui}.video-more:hover{border-color:color-mix(in srgb,var(--primary) 48%,var(--overlay-border));background:var(--surface)}
          .video-hover{position:absolute;top:40px;left:0;width:308px;padding:12px;background:var(--glass-strong);backdrop-filter:blur(16px) saturate(160%);color:var(--text);border:1px solid var(--overlay-border);border-radius:12px;box-shadow:var(--shadow-lg),inset 0 1px 0 var(--glass-rim);opacity:0;visibility:hidden;transform:translateY(-6px) scale(.98);transform-origin:top left;pointer-events:none;font-variant-numeric:tabular-nums;transition:opacity .18s cubic-bezier(.22,1,.36,1),transform .18s cubic-bezier(.22,1,.36,1),visibility 0s linear .18s}.video-hover::before{content:'';position:absolute;left:0;right:0;top:-10px;height:10px}.video-hover.above{top:auto;bottom:40px;transform:translateY(6px) scale(.98);transform-origin:bottom left}.video-hover.above::before{top:auto;bottom:-10px}.video-action-group:hover .video-hover,.video-action-group:focus-within .video-hover,.video-action-group.hover-open .video-hover{opacity:1;visibility:visible;transform:translateY(0) scale(1);pointer-events:auto;transition-delay:.04s,.04s,0s}.video-action-group.dragging .video-hover{opacity:0;visibility:hidden;pointer-events:none;transition:none}.hover-head{display:flex;align-items:flex-start;justify-content:space-between;gap:8px}.hover-title{min-width:0;display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;color:var(--text);font:650 12.5px/1.38 "Segoe UI Variable Text","Segoe UI",system-ui;overflow-wrap:anywhere}.hover-state{flex:none;padding:2px 7px;border:1px solid color-mix(in srgb,var(--green) 32%,transparent);border-radius:var(--radius-pill);background:color-mix(in srgb,var(--green) 14%,var(--surface-2));color:var(--green-ink);font:600 10px/1.3 system-ui;white-space:nowrap}.hover-state[data-tone=progress]{border-color:color-mix(in srgb,var(--primary) 34%,transparent);background:color-mix(in srgb,var(--primary) 14%,var(--surface-2));color:var(--primary-ink)}.hover-copy{margin-top:5px;color:var(--muted);font:11px/1.45 system-ui}.hover-facts{display:flex;flex-wrap:wrap;gap:4px;margin-top:8px}.hover-fact{display:inline-flex;align-items:center;height:20px;padding:0 7px;border:1px solid color-mix(in srgb,var(--border) 80%,transparent);border-radius:5px;background:var(--surface-2);color:var(--muted);font:600 10px/1 system-ui;white-space:nowrap;font-variant-numeric:tabular-nums}.hover-fact:first-child{border-color:color-mix(in srgb,var(--primary) 32%,transparent);background:color-mix(in srgb,var(--primary) 13%,var(--surface-2));color:var(--primary-ink)}.hover-source{margin-top:8px;padding:4px 7px;border-radius:6px;background:var(--surface-2);overflow:hidden;color:var(--faint);font:10px/1.35 ui-monospace,SFMono-Regular,Consolas,monospace;text-overflow:ellipsis;white-space:nowrap}.hover-qualities{display:grid;gap:5px;margin-top:9px}.hover-qualities-label{color:var(--muted);font:600 10.5px/1.2 "Segoe UI Variable Text","Segoe UI",system-ui;letter-spacing:.01em}.hover-qualities-list{display:grid;gap:4px}.quality-pill{display:flex;align-items:center;justify-content:space-between;gap:8px;width:100%;min-height:30px;padding:5px 9px;border:1px solid var(--border);border-radius:8px;background:var(--surface-2);color:var(--text);cursor:pointer;text-align:left;font:600 11.5px/1.25 "Segoe UI Variable Text","Segoe UI",system-ui;transition:background-color .15s ease,color .15s ease,border-color .15s ease,transform .12s ease,box-shadow .15s ease}.quality-pill:hover:not([disabled]){border-color:color-mix(in srgb,var(--primary) 52%,var(--border));background:color-mix(in srgb,var(--primary) 12%,var(--surface-3));transform:translateY(-1px)}.quality-pill:active:not([disabled]){transform:scale(.975)}.quality-pill.recommended{border-color:color-mix(in srgb,var(--primary) 42%,var(--border));background:color-mix(in srgb,var(--primary) 10%,var(--surface-2))}.quality-pill-main{display:inline-flex;align-items:center;gap:6px;min-width:0;flex:1}.quality-pill-name{color:var(--text);font-weight:700;white-space:nowrap}.quality-pill-meta{overflow:hidden;color:var(--muted);font:500 10.5px/1.2 "Segoe UI Variable Text","Segoe UI",system-ui;font-variant-numeric:tabular-nums;text-overflow:ellipsis;white-space:nowrap}.quality-pill-cta{flex:none;padding:2px 7px;border-radius:var(--radius-pill);background:color-mix(in srgb,var(--primary) 16%,var(--surface));color:var(--primary-ink);font:600 10px/1.3 "Segoe UI Variable Text","Segoe UI",system-ui;white-space:nowrap;transition:background-color .15s ease,color .15s ease}.quality-pill.recommended .quality-pill-cta,.quality-pill:hover:not([disabled]) .quality-pill-cta{background:var(--primary);color:var(--on-primary)}.quality-pill[disabled]{cursor:default;opacity:.6}.quality-pill[data-state=sending] .quality-pill-cta,.quality-pill[data-state=confirming] .quality-pill-cta{background:color-mix(in srgb,var(--primary) 18%,var(--surface-3));color:var(--primary-ink)}.quality-pill[data-state=queued] .quality-pill-cta,.quality-pill[data-state=done] .quality-pill-cta{background:color-mix(in srgb,var(--green) 16%,var(--surface-3));color:var(--green-ink)}.quality-pill[data-state=retry] .quality-pill-cta{background:color-mix(in srgb,var(--red) 14%,var(--surface-3));color:var(--red-ink)}.hover-actions{display:flex;gap:6px;margin-top:10px}.hover-action{min-width:0;flex:1;height:29px;padding:0 8px;border:1px solid var(--border);border-radius:var(--radius);background:var(--surface-2);color:var(--text);cursor:pointer;font:600 11px/1 "Segoe UI Variable Text","Segoe UI",system-ui}.hover-action:hover{border-color:color-mix(in srgb,var(--primary) 42%,var(--border));background:color-mix(in srgb,var(--primary) 12%,var(--surface-3))}.hover-action.primary{border-color:transparent;background:var(--primary);color:var(--on-primary);box-shadow:inset 0 1px 0 rgba(255,255,255,.22),var(--shadow-sm)}.hover-action.primary:hover{background:var(--primary-hover)}.hover-action[disabled]{cursor:default;opacity:.55}.send-error{margin-top:8px;border:1px solid color-mix(in srgb,var(--red) 32%,var(--border));border-radius:var(--radius)}button[data-state=checking]{box-shadow:inset 0 0 0 1px var(--amber,#b7791f)}button[data-state=recoverable]{box-shadow:inset 0 0 0 1px var(--red,#c53030)}button[data-state=canceled]{opacity:.8}button[data-state=sending][disabled],button[data-state=confirming][disabled],button[data-state=queued][disabled],button[data-state=done][disabled]{opacity:1;cursor:progress}button[data-state=sending]::before{content:"";display:inline-block;width:9px;height:9px;margin-right:5px;border:1.5px solid currentColor;border-right-color:transparent;border-radius:50%;vertical-align:-1px;animation:hlsd-overlay-spin .8s linear infinite}/* :is() 让这几条压过 .hover-action.primary 及其 :hover，进行中的按钮不再显示成普通蓝底。 */:is(.video-hover,.panel) button[data-state=confirming],:is(.video-hover,.panel) button[data-state=confirming]:hover{background:color-mix(in srgb,var(--primary) 14%,var(--surface-3));color:var(--primary-ink,var(--text))}:is(.video-hover,.panel) button[data-state=queued],:is(.video-hover,.panel) button[data-state=done],:is(.video-hover,.panel) button[data-state=queued]:hover,:is(.video-hover,.panel) button[data-state=done]:hover{background:color-mix(in srgb,var(--green) 14%,var(--surface-3));color:var(--green-ink)}button[data-state=done]{cursor:default}@keyframes hlsd-overlay-spin{to{transform:rotate(360deg)}}@media (prefers-reduced-motion:reduce){.video-hover,.video-hover *{transition:none!important;animation:none!important}}
          button:focus-visible{outline:2px solid var(--focus-ring, var(--primary));outline-offset:2px}
          /* 统一按钮微交互。视频悬浮按钮与面板按钮此前要么完全没有 transition，
             要么声明了 transform 过渡却没有任何状态去改 transform（等于死代码），
             点击时只剩颜色硬切。这里给它们一套与 .hlsd-button（transform .12s）和
             工作台（hover 130ms / press 90ms）对齐的反馈。 */
          .video-download,.video-more,.hover-action,.pin,.close,.download{transition:background-color .15s ease,color .15s ease,border-color .15s ease,transform .12s ease,box-shadow .15s ease}
          .video-download:hover,.video-more:hover{transform:translateY(-1px)}
          .video-download:hover{box-shadow:inset 0 1px 0 rgba(255,255,255,.28),0 6px 18px var(--shadow),0 0 0 1px rgba(0,0,0,.18)}
          .video-more:active{transform:scale(.92)}
          .video-download:active{transform:scale(.96);box-shadow:0 2px 6px var(--shadow)}
          .hover-action:active:not([disabled]),.pin:active,.close:active{transform:scale(.94)}
          .download:active:not([disabled]){transform:scale(.97)}
          /* 识别中是"还不可点"的状态，不给抬升和按压反馈，避免暗示可交互。 */
          .video-download.identifying:hover,.video-download.identifying:active{transform:none;box-shadow:var(--shadow-md)}
          /* 拖拽搬动的是 group 的 left/top，此时按钮自身的 transform 会让控件看起来在抖。 */
          .video-action-group.dragging .video-download,.video-action-group.dragging .video-more{transform:none;transition:none}
          @media(prefers-reduced-motion:reduce){*{transition:none!important;animation:none!important}}
        `
        const image = () => {
          const icon = element('img') as HTMLImageElement
          icon.src = iconUrl
          icon.alt = ''
          return icon
        }
        const panelBackdrop = element('div', 'panel-backdrop')
        panelBackdrop.setAttribute('aria-hidden', 'true')
        const panelWrap = element('div', 'wrap')
        const panel = element('div', 'panel')
        const header = element('header')
        const title = element('span', 'title')
        const titleText = element('span', 'title-text', '当前视频')
        title.append(image(), titleText)
        const headActions = element('div', 'head-actions')
        const pin = element('button', 'pin', '固定') as HTMLButtonElement
        pin.type = 'button'
        pin.title = '固定展开'
        const close = element('button', 'close', '×') as HTMLButtonElement
        close.type = 'button'
        close.title = '关闭'
        close.setAttribute('aria-label', '关闭')
        headActions.append(pin, close)
        header.append(title, headActions)
        const harvestToolbar = element('div', 'harvest-toolbar')
        harvestToolbar.hidden = true
        const harvestSearchRow = element('div', 'harvest-search-row')
        const harvestSearch = element('input', 'harvest-search') as HTMLInputElement
        harvestSearch.type = 'search'
        harvestSearch.placeholder = '搜索关键词或扩展名（如 .mp4 / .zip / 1080）'
        harvestSearch.setAttribute('aria-label', '搜索可下载链接')
        harvestSearchRow.append(harvestSearch)
        const harvestChips = element('div', 'harvest-chips')
        const harvestBatchBar = element('div', 'harvest-batch-bar')
        const harvestSelectAll = element('button', 'harvest-select-all', '全选当前筛选') as HTMLButtonElement
        harvestSelectAll.type = 'button'
        const harvestBatchBtn = element('button', 'download harvest-batch-btn', '批量下载选中 (0)') as HTMLButtonElement
        harvestBatchBtn.type = 'button'
        harvestBatchBtn.disabled = true
        harvestBatchBar.append(harvestSelectAll, harvestBatchBtn)
        harvestToolbar.append(harvestSearchRow, harvestChips, harvestBatchBar)
        const result = element('div', 'result')
        result.hidden = true
        const list = element('div', 'list')
        panel.append(header, harvestToolbar, result, list)
        panelWrap.append(panel)
        const videoButtons = element('div', 'video-buttons')
        root.append(style, panelBackdrop, panelWrap, videoButtons)
        container.append(root)
        return root
      },
    })
    ui.mount()
    const wrap = ui.shadow.querySelector<HTMLElement>('.wrap')
    const themeRoot = wrap?.parentElement
    if (themeRoot) {
      let removeTheme = applyTheme(themeRoot, 'auto')
      void browser.storage.local.get(THEME_STORAGE_KEY).then(stored => {
        if (ctx.isInvalid) return; removeTheme()
        removeTheme = applyTheme(themeRoot, normalizeThemePreference(stored[THEME_STORAGE_KEY]))
      }).catch(() => {})
      const onThemeChanged: Parameters<typeof browser.storage.onChanged.addListener>[0] = (changes, area) => {
        if (area !== 'local' || !changes[THEME_STORAGE_KEY]) return
        removeTheme()
        removeTheme = applyTheme(themeRoot, normalizeThemePreference(changes[THEME_STORAGE_KEY].newValue))
      }
      browser.storage.onChanged.addListener(onThemeChanged)
      ctx.onInvalidated(() => {
        browser.storage.onChanged.removeListener(onThemeChanged)
        removeTheme()
      })
    }
    const dragHandles = ui.shadow.querySelectorAll<HTMLElement>('header')
    let dragged = false
    let pinned = false
    let panelPosition: { x: number, y: number } | null = null
    let videoButtonPositions = new WeakMap<HTMLVideoElement, { x: number, y: number }>()
    let videoControlDragging = false
    let collapseTimer: ReturnType<typeof setTimeout> | null = null
    type VideoButtonBounds = { left: number, top: number, right: number, bottom: number }
    const videoButtonBounds = (): VideoButtonBounds => {
      const local = { left: 0, top: 0, right: innerWidth, bottom: innerHeight }
      try {
        const frame = window.frameElement as HTMLIFrameElement | null
        const parentDocument = frame?.ownerDocument
        const parentWindow = parentDocument?.defaultView
        if (!frame || !parentDocument || !parentWindow) return local

        const frameRect = frame.getBoundingClientRect()
        const frameScaleX = frameRect.width / (frame.offsetWidth || frame.clientWidth || 1)
        const frameScaleY = frameRect.height / (frame.offsetHeight || frame.clientHeight || 1)
        const content = {
          left: frameRect.left + frame.clientLeft * frameScaleX,
          top: frameRect.top + frame.clientTop * frameScaleY,
          width: frame.clientWidth * frameScaleX,
          height: frame.clientHeight * frameScaleY,
        }
        const visible = {
          left: content.left,
          top: content.top,
          right: content.left + content.width,
          bottom: content.top + content.height,
        }
        visible.left = Math.max(visible.left, 0)
        visible.top = Math.max(visible.top, 0)
        visible.right = Math.min(visible.right, parentWindow.innerWidth)
        visible.bottom = Math.min(visible.bottom, parentWindow.innerHeight)

        for (let ancestor = frame.parentElement; ancestor; ancestor = ancestor.parentElement) {
          const style = parentWindow.getComputedStyle(ancestor)
          const clipsX = style.overflowX !== 'visible'
          const clipsY = style.overflowY !== 'visible'
          if (!clipsX && !clipsY) continue
          const rect = ancestor.getBoundingClientRect()
          const scaleX = rect.width / (ancestor.offsetWidth || ancestor.clientWidth || 1)
          const scaleY = rect.height / (ancestor.offsetHeight || ancestor.clientHeight || 1)
          const left = rect.left + ancestor.clientLeft * scaleX
          const top = rect.top + ancestor.clientTop * scaleY
          if (clipsX) {
            visible.left = Math.max(visible.left, left)
            visible.right = Math.min(visible.right, left + ancestor.clientWidth * scaleX)
          }
          if (clipsY) {
            visible.top = Math.max(visible.top, top)
            visible.bottom = Math.min(visible.bottom, top + ancestor.clientHeight * scaleY)
          }
        }

        const scaleX = content.width / innerWidth
        const scaleY = content.height / innerHeight
        if (scaleX <= 0 || scaleY <= 0) return local
        return {
          left: Math.max(0, Math.min(innerWidth, (visible.left - content.left) / scaleX)),
          top: Math.max(0, Math.min(innerHeight, (visible.top - content.top) / scaleY)),
          right: Math.max(0, Math.min(innerWidth, (visible.right - content.left) / scaleX)),
          bottom: Math.max(0, Math.min(innerHeight, (visible.bottom - content.top) / scaleY)),
        }
      } catch {
        return local
      }
    }
    const clampVideoButtonPosition = (
      position: { x: number, y: number },
      size: { width: number, height: number },
      bounds: VideoButtonBounds,
    ) => {
      const visibleViewport = { width: bounds.right - bounds.left, height: bounds.bottom - bounds.top }
      const clamped = clampOverlayPosition(
        { x: position.x - bounds.left, y: position.y - bounds.top },
        size,
        visibleViewport,
        8,
      )
      return { x: clamped.x + bounds.left, y: clamped.y + bounds.top }
    }
    const fitPanel = () => {
      if (!wrap) return
      const rect = wrap.getBoundingClientRect()
      const next = clampOverlayPosition({ x: rect.left, y: rect.top }, { width: rect.width, height: rect.height }, { width: innerWidth, height: innerHeight })
      if (next.x !== rect.left || next.y !== rect.top) {
        wrap.style.left = `${next.x}px`
        wrap.style.top = `${next.y}px`
        wrap.style.right = 'auto'
      }
    }
    const panelEl = ui.shadow.querySelector<HTMLElement>('.panel')
    const panelBackdropEl = ui.shadow.querySelector<HTMLElement>('.panel-backdrop')
    const panelTitleEl = ui.shadow.querySelector<HTMLElement>('.title-text')
    const harvestToolbarEl = ui.shadow.querySelector<HTMLElement>('.harvest-toolbar')
    const harvestSearchEl = ui.shadow.querySelector<HTMLInputElement>('.harvest-search')
    const harvestChipsEl = ui.shadow.querySelector<HTMLElement>('.harvest-chips')
    const harvestSelectAllBtn = ui.shadow.querySelector<HTMLButtonElement>('.harvest-select-all')
    const harvestBatchBtn = ui.shadow.querySelector<HTMLButtonElement>('.harvest-batch-btn')
    const setPanelTitle = (label: string) => {
      if (panelTitleEl) setTextIfChanged(panelTitleEl, label)
    }
    wrap?.setAttribute('aria-hidden', 'true')
    if (panelEl && 'inert' in panelEl) (panelEl as HTMLElement & { inert: boolean }).inert = true
    const setOpen = (open: boolean) => {
      if (open && collapseTimer) {
        clearTimeout(collapseTimer)
        collapseTimer = null
      }
      wrap?.classList.toggle('open', open)
      wrap?.classList.toggle('harvest-mode', harvestMode)
      panelBackdropEl?.classList.toggle('open', open && harvestMode)
      wrap?.setAttribute('aria-hidden', String(!open))
      if (panelEl && 'inert' in panelEl) (panelEl as HTMLElement & { inert: boolean }).inert = !open
      if (open) fitPanel()
    }
    const closePanel = () => {
      if (pinned) setPinned(false)
      harvestMode = false
      setOpen(false)
    }
    const openFloatingPanel = () => {
      if (wrap && !panelPosition) {
        wrap.style.left = ''
        wrap.style.top = '16px'
        wrap.style.right = '16px'
      }
      wrap?.classList.toggle('harvest-mode', harvestMode)
      render()
      setOpen(true)
    }
    const pinButton = ui.shadow.querySelector<HTMLButtonElement>('.pin')
    const setPinned = (value: boolean) => {
      pinned = value
      pinButton?.classList.toggle('active', value)
      if (pinButton) pinButton.textContent = value ? '已固定' : '固定'
      if (value) setOpen(true)
    }
    const isEntryPendingSend = (entry?: { label: string, state?: SendState }) => Boolean(
      entry && (entry.state === 'sending' || entry.state === 'confirming' || entry.label === '等待选择'),
    )
    const hasActiveOverlaySendState = () => [...resourceSendStates.values()].some(isEntryPendingSend)
    const choicesHaveActiveSendState = (choices: MediaResource[]) => {
      const candidates = [
        ...choices,
        ...overlayQualityOptions(choices, 6).map(option => option.resource),
      ]
      return candidates.some(candidate => {
        const fp = resourceFingerprint(candidate)
        return isEntryPendingSend(resourceSendStates.get(overlaySendKey(fp, 'download')))
          || isEntryPendingSend(resourceSendStates.get(overlaySendKey(fp, 'cast')))
          || isEntryPendingSend(resourceSendStates.get(overlaySendKey(fp, 'tvbox')))
      })
    }
    wrap?.addEventListener('mouseenter', () => {
      setOpen(true)
    })
    wrap?.addEventListener('mouseleave', () => {
      // 选中链接或进行中的桌面确认不能在鼠标移出面板后消失。
      if (pinned || dragged || selectionMode || harvestMode || hasActiveOverlaySendState()) return
      collapseTimer = setTimeout(() => setOpen(false), 450)
    })
    pinButton?.addEventListener('click', () => setPinned(!pinned))
    ui.shadow.querySelector('.close')?.addEventListener('click', closePanel)
    panelBackdropEl?.addEventListener('click', event => {
      event.preventDefault()
      event.stopPropagation()
      closePanel()
    })
    ctx.addEventListener(window, 'keydown', event => {
      if (event.key === 'Escape' && wrap?.classList.contains('open')) {
        event.stopPropagation()
        closePanel()
      }
    }, { capture: true })
    harvestSearchEl?.addEventListener('input', () => {
      harvestQuery = harvestSearchEl.value
      scheduleRender()
    })
    dragHandles.forEach(handle => handle.addEventListener('pointerdown', event => {
      if (!wrap || (event.target as HTMLElement).closest('.close, .pin')) return
      if (event.button !== 0) return
      event.preventDefault()
      handle.setPointerCapture?.(event.pointerId)
      dragged = false
      const startX = event.clientX; const startY = event.clientY
      const rect = wrap.getBoundingClientRect(); const startLeft = rect.left; const startTop = rect.top
      const move = (next: PointerEvent) => {
        if (next.pointerId !== event.pointerId) return
        if (Math.abs(next.clientX - startX) + Math.abs(next.clientY - startY) > 4) dragged = true
        const position = clampOverlayPosition({ x: startLeft + next.clientX - startX, y: startTop + next.clientY - startY }, { width: rect.width, height: rect.height }, { width: innerWidth, height: innerHeight })
        wrap.style.left = `${position.x}px`
        wrap.style.top = `${position.y}px`
        wrap.style.right = 'auto'
      }
      const finish = (next: PointerEvent) => {
        if (next.pointerId !== event.pointerId) return
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', finish)
        window.removeEventListener('pointercancel', finish)
        handle.removeEventListener('lostpointercapture', finish)
        if (handle.hasPointerCapture?.(event.pointerId)) handle.releasePointerCapture?.(event.pointerId)
        panelPosition = { x: wrap.offsetLeft, y: wrap.offsetTop }
        ctx.setTimeout(() => { dragged = false }, 0)
      }
      ctx.addEventListener(window, 'pointermove', move)
      ctx.addEventListener(window, 'pointerup', finish)
      ctx.addEventListener(window, 'pointercancel', finish)
      handle.addEventListener('lostpointercapture', finish, { signal: ctx.signal })
    }))
    ctx.addEventListener(window, 'resize', fitPanel)
    if (wrap) {
      const panelResizeObserver = new ResizeObserver(() => {
        // 桌面确认结果会改变提示区高度，不能等下一次鼠标进入才把面板移回视口。
        if (wrap.classList.contains('open')) fitPanel()
      })
      panelResizeObserver.observe(wrap)
      ctx.onInvalidated(() => panelResizeObserver.disconnect())
    }

    const applySendState = (resource: MediaResource, button: HTMLButtonElement, fallbackLabel: string, action: OverlayAction = 'download') => {
      const key = overlaySendKey(resourceFingerprint(resource), action)
      if (button.dataset.sendKey !== key) button.dataset.sendKey = key
      if (button.dataset.sendFallback !== fallbackLabel) button.dataset.sendFallback = fallbackLabel
      const state = resourceSendStates.get(key)
      const labelText = state?.label || fallbackLabel
      const label = button.querySelector<HTMLElement>('.download-label')
      if (label) setTextIfChanged(label, labelText)
      else setTextIfChanged(button, labelText)
      if (state?.disabled) setAttrIfChanged(button, 'disabled', '')
      else setAttrIfChanged(button, 'disabled', null)
      if (state?.error) setAttrIfChanged(button, 'title', state.error)
      else if (button.dataset.sendError) setAttrIfChanged(button, 'title', null)
      const nextSendError = state?.error || ''
      if ((button.dataset.sendError || '') !== nextSendError) button.dataset.sendError = nextSendError
      // 与弹窗共用同一套状态名，样式按 data-state 统一（发送中/等待确认/核对中…）。
      if (state?.state) {
        if (button.dataset.state !== state.state) button.dataset.state = state.state
      } else if ('state' in button.dataset) {
        delete button.dataset.state
      }
    }

    const setSendState = (resource: MediaResource, button: HTMLButtonElement, label: string, disabled: boolean, fallbackLabel = '下载', action: OverlayAction = 'download', error = '', state?: SendState) => {
      const key = overlaySendKey(resourceFingerprint(resource), action)
      if (label) resourceSendStates.set(key, { label, disabled, error, state, url: resource.url, updatedAt: Date.now() })
      else resourceSendStates.delete(key)
      applySendState(resource, button, button.dataset.sendFallback || fallbackLabel, action)
      // 等待桌面确认期间控件可能因资源或布局更新而换代；同步当前仍在页面上的同一操作。
      const buttons = [...ui.shadow.querySelectorAll<HTMLButtonElement>('button[data-send-key]')]
        .filter(current => current.dataset.sendKey === key)
      buttons.forEach(current => applySendState(resource, current, current.dataset.sendFallback || fallbackLabel, action))
      // 播放器悬浮卡片与折叠列表是两个面板；错误必须显示在用户正在操作的卡片内。
      const hovers = new Set([button, ...buttons].map(current => current.closest('.video-action-group')?.querySelector<HTMLElement>('.video-hover')))
      hovers.forEach(hover => {
        if (!hover) return
        let detail = hover.querySelector<HTMLElement>('.send-error')
        if (error && !detail) {
          detail = document.createElement('div')
          detail.className = 'result error send-error'
          detail.setAttribute('role', 'alert')
          hover.append(detail)
        }
        if (detail) { detail.textContent = error; detail.hidden = !error }
      })
      scheduleVideoButtons()
      scheduleRender()
    }

    const sendResetTracker = createSendResetTracker({
      setTimeout: (callback, delayMs) => ctx.setTimeout(callback, delayMs),
      clearTimeout: handle => clearTimeout(handle),
    })

    const sendResource = (resource: MediaResource, button: HTMLButtonElement, forceResend = false) => {
      const key = overlaySendKey(resourceFingerprint(resource), 'download')
      const current = resourceSendStates.get(key)
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      const showResult = (text: string, error = false) => {
        if (!result) return
        result.hidden = false
        result.classList.remove('info')
        result.classList.toggle('error', error)
        result.textContent = text
      }
      const show = (state: SendState, error = '') => setSendState(
        resource, button, SEND_LABELS[state], SEND_LOCKED.has(state), '下载', 'download', error || (state === 'checking' || state === 'recoverable' ? SEND_TITLES[state] : ''), state,
      )
      const urlChanged = Boolean(current?.url && current.url !== resource.url)
      const cooldownElapsed = Boolean(current?.updatedAt && Date.now() - current.updatedAt >= 3_000)
      if (!forceResend) {
        const click = sendClickAction(current?.state, { urlChanged, cooldownElapsed })
        if ((current?.disabled && !urlChanged) || click === 'none') return
        if (click === 'open_desktop') {
          // 失败或暂停的任务在主程序里续传；不在网页上再建一个任务。
          void runtimeMessage({ type: 'activate' }).catch(() => undefined)
          showResult('已请求打开主程序，可在任务列表中继续下载')
          return
        }
      } else if (current?.state === 'sending' || current?.state === 'confirming') {
        return
      }
      const generation = sendResetTracker.nextGeneration(key)
      show('sending')
      let delivered = false
      // 重试或核对都发同一条 download-now：后台按资源摘要找回原请求编号，只回查，不重复创建；刷新重发时显式轮换请求编号。
      void withDeadline(runtimeMessage({ type: 'download-now', resource, ...(forceResend || urlChanged ? { forceResend: true } : {}) }), 10_000, '下载器响应超时，正在按原请求核对').then(async response => {
        delivered = true
        if (!sendResetTracker.isCurrent(key, generation)) return
        if (!response?.ok && response?.ownership_uncertain) {
          show('checking', response.error || SEND_TITLES.checking)
          showResult(response.error || SEND_TITLES.checking, true)
          return
        }
        if (!response?.ok) throw new Error(response?.error || '桌面端未接受请求')
        const handoff = response.handoff || {}
        const state = sendStateFromHandoff({
          status: handoff.status,
          presentation: handoff.presentation,
          taskStatus: handoff.task_status,
          taskErrorCode: handoff.task_error_code || handoff.error_code,
        })
        if (state !== 'confirming') {
          show(state)
          if (state === 'queued' || state === 'done') {
            sendResetTracker.scheduleReset(key, generation, 2_500, () => setSendState(resource, button, '', false))
          }
          return
        }
        show('confirming')
        showResult('请在桌面下载窗口确认文件名和保存位置')
        let accepted: boolean
        try {
          accepted = await waitForDownloadConfirmation(runtimeMessage, String(handoff.id || ''))
        } catch (reason) {
          if (!sendResetTracker.isCurrent(key, generation)) return
          // Core 已持久化交接：读不到结果只能“核对中”，再点按原编号回查。
          const error = reason instanceof Error ? reason.message : SEND_TITLES.checking
          show('checking', error)
          showResult(error, true)
          return
        }
        if (!sendResetTracker.isCurrent(key, generation)) return
        if (!accepted) {
          show('canceled')
          showResult('已在桌面端取消，浏览器原下载按交接结果保留')
          sendResetTracker.scheduleReset(key, generation, 2_500, () => setSendState(resource, button, '', false))
          return
        }
        show('queued')
        showResult(`已加入下载队列：${resource.filename || resource.title || resource.kind.toUpperCase()}`)
        sendResetTracker.scheduleReset(key, generation, 2_500, () => setSendState(resource, button, '', false))
      }).catch(reason => {
        if (!sendResetTracker.isCurrent(key, generation)) return
        const error = reason?.message || String(reason) || '发送失败'
        show(delivered ? 'retry' : 'checking', error)
        showResult(error, true)
      })
    }

    const pushToTv = (resource: MediaResource, button: HTMLButtonElement) => {
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      const key = overlaySendKey(resourceFingerprint(resource), 'tvbox')
      if (resourceSendStates.get(key)?.disabled) return
      const generation = sendResetTracker.nextGeneration(key)
      setSendState(resource, button, '等待选择', true, overlayActionFallback('tvbox'), 'tvbox')
      const waitForResult = async (requestId: string) => {
        const deadline = Date.now() + 130_000
        while (Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 1_000))
          const status = await runtimeMessage({ type: 'media-push-status', requestId }).catch(() => null)
          const terminal = mediaPushTerminalResult(status); if (terminal) return terminal
        }
        return { status: 'pending', message: '桌面端尚未完成设备选择' }
      }
      void runtimeMessage({ type: 'push-to-tv', resource }).then(async response => {
        if (!sendResetTracker.isCurrent(key, generation)) return
        if (!response?.ok) throw new Error(response?.error || '电视推送失败')
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = '请在桌面下载器选择 TVBox 设备' }
        const status = await waitForResult(String(response.id || ''))
        if (!sendResetTracker.isCurrent(key, generation)) return
        if (status.status !== 'done') throw new Error(status.message || '电视推送未完成')
        setSendState(resource, button, '已发送', true, overlayActionFallback('tvbox'), 'tvbox')
        if (result) result.textContent = status.message || 'TVBox 推送成功'
        sendResetTracker.scheduleReset(key, generation, 2_000, () => setSendState(resource, button, '', false, overlayActionFallback('tvbox'), 'tvbox'))
      }).catch(reason => {
        if (!sendResetTracker.isCurrent(key, generation)) return
        setSendState(resource, button, '重试', false, overlayActionFallback('tvbox'), 'tvbox', reason?.message || String(reason) || '推送失败')
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '推送失败' }
      })
    }

    const castResource = (resource: MediaResource, button: HTMLButtonElement) => {
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      const key = overlaySendKey(resourceFingerprint(resource), 'cast')
      if (resourceSendStates.get(key)?.disabled) return
      const generation = sendResetTracker.nextGeneration(key)
      setSendState(resource, button, '等待选择', true, overlayActionFallback('cast'), 'cast')
      void runtimeMessage({ type: 'cast-to-device', resource }).then(async response => {
        if (!sendResetTracker.isCurrent(key, generation)) return
        if (!response?.ok) throw new Error(response?.error || '投屏请求失败')
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = '请在桌面下载器选择投屏设备' }
        const deadline = Date.now() + 130_000
        let status: any = null
        while (Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 1_000))
          status = await runtimeMessage({ type: 'media-push-status', requestId: String(response.id || '') }).catch(() => null)
          status = mediaPushTerminalResult(status); if (status) break
        }
        if (!sendResetTracker.isCurrent(key, generation)) return
        if (status?.status !== 'done') throw new Error(status?.message || '投屏未完成')
        setSendState(resource, button, '已发送', true, overlayActionFallback('cast'), 'cast')
        if (result) result.textContent = status.message || '投屏成功'
        sendResetTracker.scheduleReset(key, generation, 2_000, () => setSendState(resource, button, '', false, overlayActionFallback('cast'), 'cast'))
      }).catch(reason => {
        if (!sendResetTracker.isCurrent(key, generation)) return
        setSendState(resource, button, '重试', false, overlayActionFallback('cast'), 'cast', reason?.message || String(reason) || '投屏请求失败')
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '投屏请求失败' }
      })
    }

    const openVideoPanel = (video: HTMLVideoElement, rect: OverlayRect) => {
      if (!wrap) return
      setPanelTitle('当前视频')
      activeVideo = video
      activePlayback = playbackByVideo.get(video) || null
      selectionMode = false
      harvestMode = false
      selectionResourceIds = null
      render()
      const preferred = panelPosition || { x: rect.right - 344, y: rect.top + 44 }
      const position = clampOverlayPosition(preferred, { width: 344, height: Math.min(520, innerHeight - 20) }, { width: innerWidth, height: innerHeight })
      panelPosition = position
      wrap.style.left = `${position.x}px`
      wrap.style.top = `${position.y}px`
      wrap.style.right = 'auto'
      setOpen(true)
    }

    interface VideoGroupState {
      video: HTMLVideoElement
      rect: OverlayRect
      choices: MediaResource[]
      matched: boolean
      oneClickChoice: boolean
      controlsWidth: number
      videoDragged: boolean
      hoverDismissTimer: ReturnType<typeof setTimeout> | null
      removeTimer: ReturnType<typeof ctx.setTimeout> | null
      button: HTMLButtonElement
      buttonLabel: HTMLSpanElement
      buttonCount: HTMLElement
      moreButton: HTMLButtonElement
      hover: HTMLDivElement
      hoverTitle: HTMLDivElement
      hoverState: HTMLSpanElement
      hoverCopy: HTMLDivElement
      hoverFacts: HTMLDivElement
      hoverSource: HTMLDivElement
      hoverQualities: HTMLDivElement
      hoverQualitiesLabel: HTMLDivElement
      hoverQualitiesList: HTMLDivElement
      hoverDownload: HTMLButtonElement
      hoverCast: HTMLButtonElement
      hoverTvbox: HTMLButtonElement
      hoverPanelBtn: HTMLButtonElement
      hoverError: HTMLDivElement
      positionHover: () => void
    }
    const videoGroupStates = new WeakMap<HTMLElement, VideoGroupState>()
    const qualityPillOptions = new WeakMap<HTMLButtonElement, OverlayQualityOption>()
    const prefersReducedMotion = () => Boolean(window.matchMedia?.('(prefers-reduced-motion: reduce)').matches)

    const createQualityPill = (_option: OverlayQualityOption): HTMLButtonElement => {
      const pill = document.createElement('button')
      pill.type = 'button'
      pill.className = 'quality-pill'
      const main = document.createElement('span')
      main.className = 'quality-pill-main'
      const name = document.createElement('span')
      name.className = 'quality-pill-name'
      const meta = document.createElement('span')
      meta.className = 'quality-pill-meta'
      main.append(name, meta)
      const cta = document.createElement('span')
      cta.className = 'quality-pill-cta download-label'
      cta.textContent = '下载'
      pill.append(main, cta)
      pill.addEventListener('click', event => {
        event.preventDefault()
        event.stopPropagation()
        const current = qualityPillOptions.get(pill)
        if (current) sendResource(current.resource, pill)
      })
      return pill
    }

    const updateQualityPill = (pill: HTMLButtonElement, option: OverlayQualityOption) => {
      qualityPillOptions.set(pill, option)
      pill.classList.toggle('recommended', option.recommended)
      setAttrIfChanged(pill, 'aria-label', option.ariaLabel)
      const name = pill.querySelector<HTMLElement>('.quality-pill-name')
      const meta = pill.querySelector<HTMLElement>('.quality-pill-meta')
      if (name) setTextIfChanged(name, option.label)
      if (meta) {
        setTextIfChanged(meta, option.detail)
        meta.hidden = !option.detail
      }
      applySendState(option.resource, pill, '下载')
    }

    type VideoRenderItem = {
      video: HTMLVideoElement
      rect: OverlayRect
      playback: PlaybackContext
      choices: MediaResource[]
      matched: boolean
    }

    const createVideoGroup = (item: VideoRenderItem, key: string): HTMLDivElement => {
      const buttonHeight = 34
      const group = document.createElement('div')
      group.className = 'video-action-group'
      const button = document.createElement('button')
      button.type = 'button'
      button.className = 'video-download'
      const icon = document.createElement('img')
      icon.src = browser.runtime.getURL('/icon-32.png')
      icon.alt = ''
      const buttonLabel = document.createElement('span')
      buttonLabel.className = 'download-label'
      const buttonCount = document.createElement('b')
      buttonCount.hidden = true
      button.append(icon, buttonLabel, buttonCount)

      const moreButton = document.createElement('button')
      moreButton.type = 'button'
      moreButton.className = 'video-more'
      moreButton.textContent = '⋯'
      moreButton.title = '展开详细面板：清晰度、投屏与 TVBox'
      moreButton.setAttribute('aria-label', '展开详细面板：清晰度、投屏与 TVBox')

      const hover = document.createElement('div')
      hover.className = 'video-hover'
      hover.setAttribute('role', 'group')
      const hoverId = `hlsd-video-hover-${key.replace(/[^a-z0-9_-]/gi, '-')}`
      hover.id = hoverId
      button.setAttribute('aria-describedby', hoverId)

      const head = document.createElement('div')
      head.className = 'hover-head'
      const hoverTitle = document.createElement('div')
      hoverTitle.className = 'hover-title'
      const hoverState = document.createElement('span')
      hoverState.className = 'hover-state'
      head.append(hoverTitle, hoverState)

      const hoverCopy = document.createElement('div')
      hoverCopy.className = 'hover-copy'
      hoverCopy.hidden = true

      const hoverFacts = document.createElement('div')
      hoverFacts.className = 'hover-facts'

      const hoverSource = document.createElement('div')
      hoverSource.className = 'hover-source'

      const hoverQualities = document.createElement('div')
      hoverQualities.className = 'hover-qualities'
      hoverQualities.hidden = true
      const hoverQualitiesLabel = document.createElement('div')
      hoverQualitiesLabel.className = 'hover-qualities-label'
      hoverQualitiesLabel.textContent = '一键下载清晰度'
      const hoverQualitiesList = document.createElement('div')
      hoverQualitiesList.className = 'hover-qualities-list'
      hoverQualities.append(hoverQualitiesLabel, hoverQualitiesList)

      const hoverActions = document.createElement('div')
      hoverActions.className = 'hover-actions'
      const hoverDownload = document.createElement('button')
      hoverDownload.type = 'button'
      hoverDownload.className = 'hover-action primary'
      hoverDownload.textContent = '下载'
      const hoverCast = document.createElement('button')
      hoverCast.type = 'button'
      hoverCast.className = 'hover-action'
      hoverCast.textContent = '投屏'
      const hoverTvbox = document.createElement('button')
      hoverTvbox.type = 'button'
      hoverTvbox.className = 'hover-action'
      hoverTvbox.textContent = 'TVBox'
      const hoverPanelBtn = document.createElement('button')
      hoverPanelBtn.type = 'button'
      hoverPanelBtn.className = 'hover-action'
      hoverPanelBtn.textContent = '更多'
      hoverActions.append(hoverDownload, hoverCast, hoverTvbox, hoverPanelBtn)

      const hoverError = document.createElement('div')
      hoverError.className = 'result error send-error'
      hoverError.setAttribute('role', 'alert')
      hoverError.hidden = true

      hover.append(head, hoverCopy, hoverFacts, hoverSource, hoverQualities, hoverActions, hoverError)
      group.append(button, moreButton, hover)

      const positionHover = () => {
        const groupRect = group.getBoundingClientRect()
        const hoverWidth = Math.max(220, Math.min(308, innerWidth - 16))
        hover.style.width = `${hoverWidth}px`
        const relativeLeft = Math.max(8 - groupRect.left, Math.min(0, innerWidth - 8 - hoverWidth - groupRect.left))
        hover.style.left = `${relativeLeft}px`
        ctx.requestAnimationFrame(() => {
          const hoverHeight = hover.offsetHeight || 160
          hover.classList.toggle('above', groupRect.bottom + 6 + hoverHeight > innerHeight - 8 && groupRect.top >= hoverHeight + 8)
        })
      }

      const state: VideoGroupState = {
        video: item.video,
        rect: item.rect,
        choices: item.choices,
        matched: item.matched,
        oneClickChoice: item.matched && item.choices.length === 1,
        controlsWidth: 194,
        videoDragged: false,
        hoverDismissTimer: null,
        removeTimer: null,
        button,
        buttonLabel,
        buttonCount,
        moreButton,
        hover,
        hoverTitle,
        hoverState,
        hoverCopy,
        hoverFacts,
        hoverSource,
        hoverQualities,
        hoverQualitiesLabel,
        hoverQualitiesList,
        hoverDownload,
        hoverCast,
        hoverTvbox,
        hoverPanelBtn,
        hoverError,
        positionHover,
      }
      videoGroupStates.set(group, state)

      button.addEventListener('pointerdown', event => {
        if (event.button !== 0) return
        event.preventDefault()
        event.stopPropagation()
        button.setPointerCapture(event.pointerId)
        state.videoDragged = false
        videoControlDragging = true
        const startX = event.clientX
        const startY = event.clientY
        const startLeft = group.offsetLeft
        const startTop = group.offsetTop
        const move = (next: PointerEvent) => {
          if (next.pointerId !== event.pointerId) return
          if (!state.videoDragged && Math.abs(next.clientX - startX) + Math.abs(next.clientY - startY) > 4) {
            state.videoDragged = true
            group.classList.add('dragging')
          }
          if (!state.videoDragged) return
          const height = button.offsetHeight || buttonHeight
          const position = clampVideoButtonPosition(
            { x: startLeft + next.clientX - startX, y: startTop + next.clientY - startY },
            { width: state.controlsWidth, height },
            videoButtonBounds(),
          )
          group.style.left = `${position.x}px`
          group.style.top = `${position.y}px`
        }
        const finish = (next: PointerEvent) => {
          if (next.pointerId !== event.pointerId) return
          window.removeEventListener('pointermove', move, true)
          window.removeEventListener('pointerup', finish, true)
          window.removeEventListener('pointercancel', finish, true)
          button.removeEventListener('lostpointercapture', finish)
          if (state.videoDragged && next.type !== 'pointercancel') {
            videoButtonPositions.set(state.video, { x: group.offsetLeft, y: group.offsetTop })
          }
          videoControlDragging = false
          group.classList.remove('dragging')
          if (button.hasPointerCapture(event.pointerId)) button.releasePointerCapture(event.pointerId)
          scheduleVideoButtons()
        }
        // Use the window capture phase as a fallback for players that stop
        // dispatching pointer events from their overlay while the pointer is
        // moved outside the button. Pointer capture alone is not reliable in
        // every iframe/player combination.
        ctx.addEventListener(window, 'pointermove', move, { capture: true })
        ctx.addEventListener(window, 'pointerup', finish, { capture: true })
        ctx.addEventListener(window, 'pointercancel', finish, { capture: true })
        // iframe 切换或浏览器撤销捕获时，pointerup 可能不再送达当前页面。
        button.addEventListener('lostpointercapture', finish)
      })

      button.addEventListener('click', event => {
        if (state.videoDragged) {
          event.preventDefault()
          event.stopImmediatePropagation()
          state.videoDragged = false
          return
        }
        if (state.oneClickChoice && state.choices[0]) {
          event.preventDefault()
          event.stopImmediatePropagation()
          sendResource(state.choices[0], button)
          return
        }
        // Multi-stream players still open the full evidence panel on main click,
        // while hovering provides 1-click inline quality buttons immediately.
        openVideoPanel(state.video, state.rect)
      })

      moreButton.addEventListener('pointerdown', event => event.stopPropagation())
      moreButton.addEventListener('click', event => {
        event.preventDefault()
        event.stopPropagation()
        openVideoPanel(state.video, state.rect)
      })

      hoverDownload.addEventListener('click', event => {
        event.preventDefault()
        event.stopPropagation()
        if (state.choices[0]) sendResource(state.choices[0], hoverDownload)
      })
      hoverCast.addEventListener('click', event => {
        event.preventDefault()
        event.stopPropagation()
        if (state.choices[0]) castResource(state.choices[0], hoverCast)
      })
      hoverTvbox.addEventListener('click', event => {
        event.preventDefault()
        event.stopPropagation()
        if (state.choices[0]) pushToTv(state.choices[0], hoverTvbox)
      })
      hoverPanelBtn.addEventListener('click', event => {
        event.preventDefault()
        event.stopPropagation()
        openVideoPanel(state.video, state.rect)
      })

      const keepHoverOpen = () => {
        if (playerDismissTimer) clearTimeout(playerDismissTimer)
        playerDismissTimer = undefined
        if (state.hoverDismissTimer) clearTimeout(state.hoverDismissTimer)
        state.hoverDismissTimer = null
        group.classList.add('hover-open')
        positionHover()
      }
      const dismissHover = () => {
        if (state.hoverDismissTimer) clearTimeout(state.hoverDismissTimer)
        state.hoverDismissTimer = setTimeout(() => {
          state.hoverDismissTimer = null
          if (shouldKeepHoverOpen(group.matches(':hover'), group.contains(ui.shadow.activeElement))) return
          group.classList.remove('hover-open')
          updateHoveredPlayer()
          scheduleVideoButtons()
        }, HOVER_DISMISS_DELAY_MS)
      }
      group.addEventListener('pointerenter', keepHoverOpen)
      group.addEventListener('pointerleave', dismissHover)
      group.addEventListener('focusin', keepHoverOpen)
      group.addEventListener('focusout', dismissHover)

      if (prefersReducedMotion()) {
        group.classList.add('visible')
      } else {
        ctx.requestAnimationFrame(() => {
          if (group.isConnected && group.getAttribute('data-leaving') !== '1') {
            group.classList.add('visible')
            positionHover()
          }
        })
      }
      return group
    }

    const updateVideoGroup = (group: HTMLDivElement, item: VideoRenderItem, buttonBounds: VideoButtonBounds) => {
      const state = videoGroupStates.get(group)
      if (!state) return
      if (state.removeTimer) {
        clearTimeout(state.removeTimer)
        state.removeTimer = null
      }
      if (group.getAttribute('data-leaving') === '1') group.removeAttribute('data-leaving')
      if (group.isConnected) group.classList.add('visible')

      const { video, rect, choices, matched } = item
      // playerPlaybackResources already enforces the ambiguity boundary for
      // blob/MSE (per-SourceBuffer evidence).
      const oneClickChoice = matched && choices.length === 1
      const buttonWidth = 156
      const buttonHeight = 34
      const controlsWidth = buttonWidth + (oneClickChoice ? 38 : 0)
      state.video = video
      state.rect = rect
      state.choices = choices
      state.matched = matched
      state.oneClickChoice = oneClickChoice
      state.controlsWidth = controlsWidth

      const besidePlayer = rect.right + 8
      const defaultLeft = besidePlayer + controlsWidth <= buttonBounds.right - 8
        ? besidePlayer
        : Math.max(buttonBounds.left + 8, Math.min(rect.right - controlsWidth - 8, buttonBounds.right - controlsWidth - 8))
      const defaultTop = Math.max(buttonBounds.top + 8, Math.min(rect.top + 8, buttonBounds.bottom - buttonHeight - 8))
      const saved = videoButtonPositions.get(video)
      const position = clampVideoButtonPosition(saved || { x: defaultLeft, y: defaultTop },
        { width: controlsWidth, height: buttonHeight }, buttonBounds)
      const leftPx = `${position.x}px`
      const topPx = `${position.y}px`
      if (group.style.left !== leftPx) group.style.left = leftPx
      if (group.style.top !== topPx) group.style.top = topPx

      const primaryChoice = choices[0]
      const fallbackLabel = oneClickChoice ? '下载视频' : '选择资源'
      if (oneClickChoice && primaryChoice) {
        if (state.button.dataset.resourceId !== primaryChoice.id) state.button.dataset.resourceId = primaryChoice.id
        setAttrIfChanged(state.button, 'aria-label', '下载当前视频')
        applySendState(primaryChoice, state.button, fallbackLabel)
      } else {
        delete state.button.dataset.resourceId
        delete state.button.dataset.sendKey
        delete state.button.dataset.sendFallback
        delete state.button.dataset.sendError
        delete state.button.dataset.state
        setAttrIfChanged(state.button, 'disabled', null)
        setAttrIfChanged(state.button, 'aria-label', `选择 ${choices.length} 个视频资源`)
        setTextIfChanged(state.buttonLabel, fallbackLabel)
      }
      state.buttonCount.hidden = choices.length <= 1
      if (choices.length > 1) setTextIfChanged(state.buttonCount, String(choices.length))
      state.moreButton.hidden = !oneClickChoice

      const details = overlayResourceDetails(primaryChoice)
      const qualityOptions = overlayQualityOptions(choices, 6)
      const hasInlineQualities = qualityOptions.length > 0
      const primarySendState = primaryChoice
        ? resourceSendStates.get(overlaySendKey(resourceFingerprint(primaryChoice), 'download'))?.state
        : undefined
      const hoverProgressLabel = primarySendState === 'sending'
        ? '发送中'
        : primarySendState === 'confirming'
          ? '等待桌面确认'
          : primarySendState === 'queued'
            ? '已加入队列'
            : primarySendState === 'done'
              ? '已完成'
              : ''

      setTextIfChanged(state.hoverTitle, oneClickChoice || hasInlineQualities ? details.title : `${choices.length} 个可用资源`)
      setTextIfChanged(
        state.hoverState,
        hoverProgressLabel || (hasInlineQualities ? `${qualityOptions.length} 个清晰度` : oneClickChoice ? details.state : '需要选择'),
      )
      setAttrIfChanged(
        state.hoverState,
        'data-tone',
        primarySendState === 'sending' || primarySendState === 'confirming' ? 'progress' : null,
      )
      const copyText = !oneClickChoice && !hasInlineQualities
        ? (matched ? `优先候选：${details.title}` : '已发现页面媒体，但尚未确认播放器归属。请核对标题、清晰度和来源后选择。')
        : ''
      state.hoverCopy.hidden = !copyText
      if (copyText) setTextIfChanged(state.hoverCopy, copyText)

      const factItems = oneClickChoice || hasInlineQualities ? details.facts : details.facts.slice(0, 4)
      patchKeyedChildren(
        state.hoverFacts,
        factItems,
        value => value,
        () => {
          const fact = document.createElement('span')
          fact.className = 'hover-fact'
          return fact
        },
        (fact, value) => setTextIfChanged(fact, value),
      )

      state.hoverSource.hidden = !oneClickChoice && !hasInlineQualities
      if (!state.hoverSource.hidden) {
        setTextIfChanged(state.hoverSource, details.source)
        setAttrIfChanged(state.hoverSource, 'title', details.source)
      }

      state.hoverQualities.hidden = !hasInlineQualities
      patchKeyedChildren(
        state.hoverQualitiesList,
        qualityOptions,
        option => option.key,
        createQualityPill,
        updateQualityPill,
      )

      const playableChoice = Boolean(primaryChoice && isPlayableMediaResource(primaryChoice) && !primaryChoice.drmProtected)
      state.hoverDownload.hidden = !oneClickChoice || hasInlineQualities || Boolean(primaryChoice?.drmProtected)
      state.hoverCast.hidden = !matched || !playableChoice
      state.hoverTvbox.hidden = !matched || !playableChoice || tvboxConfigured === false
      state.hoverPanelBtn.className = !oneClickChoice && !hasInlineQualities ? 'hover-action primary' : 'hover-action'
      setTextIfChanged(state.hoverPanelBtn, !oneClickChoice && !hasInlineQualities ? '查看并选择' : '更多')
      if (primaryChoice) {
        if (!state.hoverDownload.hidden) applySendState(primaryChoice, state.hoverDownload, '下载')
        if (!state.hoverCast.hidden) applySendState(primaryChoice, state.hoverCast, '投屏', 'cast')
        if (!state.hoverTvbox.hidden) applySendState(primaryChoice, state.hoverTvbox, 'TVBox', 'tvbox')
      }

      const errorKeys = [
        resourceFingerprint(primaryChoice),
        overlaySendKey(resourceFingerprint(primaryChoice), 'cast'),
        overlaySendKey(resourceFingerprint(primaryChoice), 'tvbox'),
        ...qualityOptions.map(option => resourceFingerprint(option.resource)),
      ]
      const error = errorKeys.map(key => resourceSendStates.get(key)?.error || '').find(Boolean) || ''
      setTextIfChanged(state.hoverError, error)
      state.hoverError.hidden = !error

      state.positionHover()
    }

    const confirmPlaybackChoices = (playback: PlaybackContext | null): MediaResource[] => {
      if (!playback) return []
      const candidates = playerPlaybackResources([...resources.values()], playback, 8)
      const exact = candidates.filter(resource => playback.sourceUrls.some(source => resourceMatchesPlaybackSource(resource, source)))
      const choices = (exact.length ? exact : candidates)
        .sort((left, right) => resourceRank(right) - resourceRank(left) || (right.height || 0) - (left.height || 0) || (right.bandwidth || 0) - (left.bandwidth || 0) || (right.size || right.estimatedSize || 0) - (left.size || left.estimatedSize || 0))
      for (const resource of choices) {
        if (resource.evidence?.includes('playback_confirmed')) continue
        const confirmed = { ...resource, evidence: [...(resource.evidence || []), 'playback_confirmed'].slice(-16) }
        resources.set(resourceFingerprint(resource), confirmed)
        const write = runtimeMessage({ type: 'resource', resource: confirmed }).catch(() => undefined)
        pendingResourceWrites.add(write)
        void write.finally(() => pendingResourceWrites.delete(write))
      }
      return choices
    }

    const confirmAllKnownVideoPlaybacks = () => {
      for (const video of currentVideos()) {
        const playback = playbackByVideo.get(video) || null
        if (playback) confirmPlaybackChoices(playback)
      }
    }

    const updateVideoButtons = () => {
      if (ctx.isInvalid) return
      const layer = ui.shadow.querySelector<HTMLElement>('.video-buttons')
      if (!layer) return
      // Players emit timeupdate while the pointer is down. Replacing the
      // control in that interval cancels pointer capture before it can move.
      if (videoControlDragging) return
      const siteExcluded = isCurrentSiteExcluded()
      const buttonBounds = videoButtonBounds()
      const existingGroups = new Map<string, HTMLElement>()
      for (const node of layer.querySelectorAll<HTMLElement>('.video-action-group')) {
        if (node.getAttribute('data-leaving') === '1') continue
        const state = videoGroupStates.get(node)
        if (state) existingGroups.set(mediaOwner(state.video), node)
      }
      const matchedVideos = siteExcluded
        ? []
        : currentVideos()
            .flatMap(video => {
              const rect = visibleMediaRect(video, { width: innerWidth, height: innerHeight })
              return rect ? [{ video, rect, playback: playbackByVideo.get(video) || null }] : []
            })
            .filter((item): item is { video: HTMLVideoElement; rect: OverlayRect; playback: PlaybackContext } => Boolean(item.playback)
              && (item.video === activeVideo || (!item.video.paused && !item.video.ended))
              && item.rect.width >= 180 && item.rect.height >= 100)
            .map(item => {
              const allChoices = confirmPlaybackChoices(item.playback)
              const choices = allChoices.filter(resource => !resource.drmProtected)
              const matched = choices.length > 0
              return { ...item, choices, matched }
            })
            .filter(({ choices }) => shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: choices.length }))
      const videos = matchedVideos
        .filter(({ video, choices }) => {
          if (video === hoveredVideo) return true
          const group = existingGroups.get(mediaOwner(video))
          if (group && (group.classList.contains('hover-open') || group.matches(':hover') || group.contains(ui.shadow.activeElement))) {
            return true
          }
          return choicesHaveActiveSendState(choices)
        })
        .sort((left, right) => Number(right.video === activeVideo) - Number(left.video === activeVideo)
          || right.rect.width * right.rect.height - left.rect.width * left.rect.height)
      const signature = JSON.stringify([
        innerWidth, innerHeight, buttonBounds, selectionMode, siteExcluded, tvboxConfigured, [...resourceSendStates],
        matchedVideos.length,
        videos.map(({ video, rect, playback, choices, matched }) => [
          mediaOwner(video), rect.x, rect.y, rect.width, rect.height, playback.sourceUrls, playback.startedAt,
          resourcePresentationSignature(choices, false), matched, videoButtonPositions.get(video),
        ]),
      ])
      // 时间进度事件不能不断重排或替换按钮；只在资源、布局或发送状态变化时原地修补。
      if (signature === renderedVideoSignature) return
      renderedVideoSignature = signature
      // 每个播放器独立匹配自己的 currentSrc 或 SourceBuffer 响应，按 mediaOwner 原地修补 DOM。
      patchKeyedChildren(
        layer,
        videos,
        ({ video }) => mediaOwner(video),
        (item, key) => createVideoGroup(item, key),
        (group, item) => updateVideoGroup(group, item, buttonBounds),
        {
          onRemove(group) {
            const state = videoGroupStates.get(group)
            if (state?.hoverDismissTimer) {
              clearTimeout(state.hoverDismissTimer)
              state.hoverDismissTimer = null
            }
            if (prefersReducedMotion()) {
              group.remove()
              return
            }
            group.setAttribute('data-leaving', '1')
            group.classList.remove('visible', 'hover-open')
            if (state) {
              if (state.removeTimer) clearTimeout(state.removeTimer)
              state.removeTimer = ctx.setTimeout(() => {
                state.removeTimer = null
                if (group.getAttribute('data-leaving') === '1') group.remove()
              }, 190)
            } else {
              group.remove()
            }
          },
        },
      )
      if (siteExcluded && !selectionMode && !harvestMode) {
        if (pinned) setPinned(false)
        setOpen(false)
        panelPosition = null
      } else if (!matchedVideos.length && !pinned && !selectionMode && !harvestMode && !hasActiveOverlaySendState()) {
        setOpen(false)
        panelPosition = null
      }
    }

    interface PanelRowState {
      resource: MediaResource
      selected: MediaResource
      checkbox: HTMLInputElement
      name: HTMLDivElement
      kind: HTMLDivElement
      resourceUrl: HTMLElement
      select: HTMLSelectElement
      downloadBtn: HTMLButtonElement
      resendBtn: HTMLButtonElement
      castBtn: HTMLButtonElement
      pushBtn: HTMLButtonElement
    }
    const panelRowStates = new WeakMap<HTMLElement, PanelRowState>()
    let currentFilteredHarvestEntries: MediaResource[] = []
    let currentAllHarvestEntries: MediaResource[] = []

    harvestSelectAllBtn?.addEventListener('click', () => {
      if (!currentFilteredHarvestEntries.length) return
      const allChecked = currentFilteredHarvestEntries.every(item => harvestCheckedIds.has(resourceFingerprint(item)))
      for (const item of currentFilteredHarvestEntries) {
        const fp = resourceFingerprint(item)
        if (allChecked) harvestCheckedIds.delete(fp)
        else harvestCheckedIds.add(fp)
      }
      scheduleRender()
    })

    harvestBatchBtn?.addEventListener('click', () => {
      if (harvestBatchBusy) return
      const selectedItems = currentAllHarvestEntries
        .filter(item => harvestCheckedIds.has(resourceFingerprint(item)))
        .map(item => {
          const pref = preferredVariants.get(resourceFingerprint(item))
          if (!pref || !item.variants?.length) return item
          const idx = pref.variantId
            ? item.variants.findIndex(v => v.variantId === pref.variantId)
            : item.variants.findIndex(v => (v.height || 0) === pref.height && (v.bandwidth || 0) === pref.bandwidth)
          return idx >= 0 ? selectMediaVariant(item, item.variants[idx]) : item
        })
      if (!selectedItems.length) return
      harvestBatchBusy = true
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      if (result) {
        result.hidden = false
        result.classList.remove('error', 'info')
        result.dataset.source = 'batch'
        result.textContent = `正在批量发送 ${selectedItems.length} 个链接到桌面端…`
      }
      scheduleRender()
      void withDeadline(runtimeMessage({ type: 'download-batch', resources: selectedItems }), 20_000, '批量发送响应超时').then(response => {
        harvestBatchBusy = false
        const sent = Number(response?.sent || 0)
        const failed = Number(response?.failed || 0)
        if (result) {
          result.hidden = false
          result.classList.remove('info')
          result.classList.toggle('error', sent === 0 && failed > 0)
          result.dataset.source = 'batch'
          result.textContent = failed > 0
            ? `批量发送完成：已处理 ${sent} 个，${failed} 个未能加入队列`
            : `已发送 ${sent} 个链接，请在桌面下载器确认或查看队列`
        }
        scheduleRender()
      }).catch(reason => {
        harvestBatchBusy = false
        if (result) {
          result.hidden = false
          result.classList.remove('info')
          result.classList.add('error')
          result.dataset.source = 'batch'
          result.textContent = reason?.message || String(reason) || '批量下载失败'
        }
        scheduleRender()
      })
    })

    const createPanelRow = (resource: MediaResource): HTMLDivElement => {
      const row = document.createElement('div')
      row.className = 'item'
      const itemHead = document.createElement('div')
      itemHead.className = 'item-head'
      const checkbox = document.createElement('input')
      checkbox.type = 'checkbox'
      checkbox.className = 'item-check'
      checkbox.hidden = true
      checkbox.setAttribute('aria-label', '选择此链接用于批量下载')
      const meta = document.createElement('div')
      meta.className = 'meta'
      const name = document.createElement('div')
      name.className = 'name'
      const kind = document.createElement('div')
      kind.className = 'kind'
      const resourceUrl = document.createElement('code')
      resourceUrl.className = 'resource-url'
      const select = document.createElement('select')
      select.className = 'hlsd-select quality-select'
      select.setAttribute('aria-label', '选择视频清晰度')
      select.hidden = true
      meta.append(name, kind, resourceUrl, select)
      itemHead.append(checkbox, meta)

      const actions = document.createElement('div')
      actions.className = 'item-actions'
      const downloadBtn = document.createElement('button')
      downloadBtn.type = 'button'
      downloadBtn.className = 'download'
      downloadBtn.textContent = '下载'
      const resendBtn = document.createElement('button')
      resendBtn.type = 'button'
      resendBtn.className = 'download resend'
      resendBtn.textContent = '刷新重发'
      resendBtn.title = '使用最新捕获的媒体地址重新发送或刷新桌面任务'
      resendBtn.hidden = true
      const castBtn = document.createElement('button')
      castBtn.type = 'button'
      castBtn.className = 'download cast'
      castBtn.textContent = '投屏'
      castBtn.title = '直接投屏当前媒体链接到 DLNA 或 Chromecast'
      const pushBtn = document.createElement('button')
      pushBtn.type = 'button'
      pushBtn.className = 'download push-tv'
      pushBtn.textContent = 'TVBox'
      pushBtn.title = '直接推送当前媒体链接到 TVBox'
      actions.append(downloadBtn, resendBtn, castBtn, pushBtn)
      row.append(itemHead, actions)

      const rowState: PanelRowState = {
        resource,
        selected: resource,
        checkbox,
        name,
        kind,
        resourceUrl,
        select,
        downloadBtn,
        resendBtn,
        castBtn,
        pushBtn,
      }
      panelRowStates.set(row, rowState)

      checkbox.addEventListener('change', () => {
        const fp = resourceFingerprint(rowState.resource)
        if (checkbox.checked) harvestCheckedIds.add(fp)
        else harvestCheckedIds.delete(fp)
        scheduleRender()
      })
      downloadBtn.addEventListener('click', () => sendResource(rowState.selected, downloadBtn))
      resendBtn.addEventListener('click', () => sendResource(rowState.selected, downloadBtn, true))
      castBtn.addEventListener('click', () => castResource(rowState.selected, castBtn))
      pushBtn.addEventListener('click', () => pushToTv(rowState.selected, pushBtn))
      select.addEventListener('change', () => {
        const current = rowState.resource
        const variant = current.variants?.[Number(select.value)]
        rowState.selected = variant ? selectMediaVariant(current, variant) : current
        const fp = resourceFingerprint(current)
        if (variant) {
          preferredVariants.set(fp, {
            ...(variant.variantId ? { variantId: variant.variantId } : {}),
            height: variant.height || 0,
            bandwidth: variant.bandwidth || 0,
          })
        } else {
          preferredVariants.delete(fp)
        }
        const playable = isPlayableMediaResource(rowState.selected) && !rowState.selected.drmProtected
        castBtn.hidden = !playable
        pushBtn.hidden = !playable || tvboxConfigured === false
        if (rowState.selected.drmProtected) {
          downloadBtn.disabled = true
          setTextIfChanged(downloadBtn, 'DRM 加密保护')
          setAttrIfChanged(downloadBtn, 'title', 'DRM 加密保护 · 不支持直接下载')
          resendBtn.hidden = true
        } else {
          applySendState(rowState.selected, downloadBtn, '下载')
          const sendState = resourceSendStates.get(overlaySendKey(resourceFingerprint(rowState.selected), 'download'))?.state
          resendBtn.hidden = !canForceResend(sendState)
        }
        if (playable) {
          if (!pushBtn.hidden) applySendState(rowState.selected, pushBtn, overlayActionFallback('tvbox'), 'tvbox')
          applySendState(rowState.selected, castBtn, overlayActionFallback('cast'), 'cast')
        }
      })

      return row
    }

    const updatePanelRow = (row: HTMLDivElement, resource: MediaResource) => {
      const rowState = panelRowStates.get(row)
      if (!rowState) return
      rowState.resource = resource
      const fp = resourceFingerprint(resource)
      rowState.checkbox.hidden = !harvestMode
      rowState.checkbox.checked = harvestMode && harvestCheckedIds.has(fp)
      const titleText = resource.title || resource.filename || resource.url.split('/').pop() || resource.url
      setTextIfChanged(rowState.name, titleText)
      setAttrIfChanged(rowState.name, 'title', resource.title || resource.filename || resource.url)
      let host = ''
      try { host = new URL(resource.url).host } catch {}
      const quality = resource.quality || resourceQuality(resource.url, resource.height)
      const streamMode = resource.isLive === true ? '直播' : ''
      const duration = resource.duration ? formatDuration(resource.duration) : ''
      const bandwidth = resource.bandwidth ? formatBitrate(resource.bandwidth) : ''
      const likelySize = resource.size || resource.estimatedSize || 0
      const sizeLabel = resource.size ? formatBytes(resource.size) : likelySize ? `约 ${formatBytes(likelySize)}` : '大小未知'
      const kindText = [resource.kind.toUpperCase(), streamMode, quality, resource.width && resource.height ? `${resource.width}×${resource.height}` : '', bandwidth, duration, sizeLabel, host].filter(Boolean).join(' · ')
      setTextIfChanged(rowState.kind, kindText)
      const resourceLocation = safeResourceLocation(resource.url)
      setTextIfChanged(rowState.resourceUrl, resourceLocation)
      setAttrIfChanged(rowState.resourceUrl, 'title', resourceLocation)

      const preference = preferredVariants.get(fp)
      const selectedIndex = preference && resource.variants?.length
        ? (() => {
          if (preference.variantId) {
            const byId = resource.variants.findIndex(variant => variant.variantId === preference.variantId)
            if (byId >= 0) return byId
          }
          return resource.variants.findIndex(variant => (variant.height || 0) === preference.height
            && (variant.bandwidth || 0) === preference.bandwidth)
        })()
        : -1
      const selected = selectedIndex >= 0 ? selectMediaVariant(resource, resource.variants![selectedIndex]) : resource
      rowState.selected = selected

      if (resource.variants?.length && !resource.drmProtected) {
        rowState.select.hidden = false
        syncSelectOptions(
          rowState.select,
          [
            { value: 'auto', label: '自动（最高）' },
            ...resource.variants.map((variant, index) => ({
              value: String(index),
              label: [variant.quality || (variant.height ? `${variant.height}p` : '线路'), variant.bandwidth ? formatBitrate(variant.bandwidth) : ''].filter(Boolean).join(' · '),
            })),
          ],
          selectedIndex >= 0 ? String(selectedIndex) : 'auto',
        )
      } else {
        rowState.select.hidden = true
      }

      const playable = isPlayableMediaResource(selected) && !selected.drmProtected
      rowState.castBtn.hidden = !playable
      rowState.pushBtn.hidden = !playable || tvboxConfigured === false
      if (selected.drmProtected) {
        rowState.downloadBtn.disabled = true
        setTextIfChanged(rowState.downloadBtn, 'DRM 加密保护')
        setAttrIfChanged(rowState.downloadBtn, 'title', 'DRM 加密保护 · 不支持直接下载')
        rowState.resendBtn.hidden = true
      } else {
        applySendState(selected, rowState.downloadBtn, '下载')
        const sendState = resourceSendStates.get(overlaySendKey(resourceFingerprint(selected), 'download'))?.state
        rowState.resendBtn.hidden = !canForceResend(sendState)
      }
      if (playable) {
        applySendState(selected, rowState.castBtn, overlayActionFallback('cast'), 'cast')
        if (!rowState.pushBtn.hidden) applySendState(selected, rowState.pushBtn, overlayActionFallback('tvbox'), 'tvbox')
      }
    }

    const render = () => {
      if (ctx.isInvalid) return
      const list = ui.shadow.querySelector('.list')
      if (!list) return
      wrap?.classList.toggle('harvest-mode', harvestMode)
      if (panelBackdropEl && wrap) {
        panelBackdropEl.classList.toggle('open', harvestMode && wrap.classList.contains('open'))
      }
      const selectionResources = selectionResourceIds
        ? [...resources.values()].filter(resource => selectionResourceIds?.has(resourceFingerprint(resource)))
        : [...resources.values()]
      // 已有播放器归属证据时，展开面板必须沿用同一资源，不能重新按全页码率挑选。
      const matched = playerPlaybackResources([...resources.values()], activePlayback, 8)
      const allSelectionEntries = selectionMode && !activePlayback
        ? compactResources(selectionResources, 200).slice(0, 200)
        : []
      const entries = selectionMode && !activePlayback
        ? (harvestMode
            ? filterHarvestResources(allSelectionEntries, harvestCategory, harvestQuery)
            : allSelectionEntries.slice(0, 40))
        : matched.length ? matched : visiblePlaybackResources([...resources.values()], activePlayback, 8)

      currentAllHarvestEntries = harvestMode ? allSelectionEntries : []
      currentFilteredHarvestEntries = harvestMode ? entries : []

      if (harvestToolbarEl) {
        harvestToolbarEl.hidden = !harvestMode
        if (harvestMode && harvestChipsEl) {
          const categoryCounts = countHarvestCategories(allSelectionEntries)
          patchKeyedChildren(
            harvestChipsEl,
            [...HARVEST_CATEGORY_CHIPS],
            ([category]) => category,
            ([category]) => {
              const chip = document.createElement('button')
              chip.type = 'button'
              chip.className = 'harvest-chip'
              const labelSpan = document.createElement('span')
              const countBadge = document.createElement('b')
              chip.append(labelSpan, countBadge)
              chip.addEventListener('click', () => {
                harvestCategory = category
                scheduleRender()
              })
              return chip
            },
            (chip, [category, label]) => {
              setAttrIfChanged(chip, 'aria-pressed', String(harvestCategory === category))
              const labelSpan = chip.querySelector('span')
              const countBadge = chip.querySelector('b')
              if (labelSpan) setTextIfChanged(labelSpan, label)
              if (countBadge) setTextIfChanged(countBadge, String(categoryCounts[category]))
            },
          )
          const checkedCount = allSelectionEntries.filter(item => harvestCheckedIds.has(resourceFingerprint(item))).length
          const allFilteredSelected = entries.length > 0 && entries.every(item => harvestCheckedIds.has(resourceFingerprint(item)))
          if (harvestSelectAllBtn) {
            harvestSelectAllBtn.disabled = entries.length === 0
            setAttrIfChanged(harvestSelectAllBtn, 'aria-pressed', String(allFilteredSelected))
            setTextIfChanged(harvestSelectAllBtn, allFilteredSelected ? '取消全选' : '全选当前筛选')
          }
          if (harvestBatchBtn) {
            harvestBatchBtn.disabled = checkedCount === 0 || harvestBatchBusy
            setTextIfChanged(harvestBatchBtn, harvestBatchBusy ? `发送中 (${checkedCount})…` : `批量下载选中 (${checkedCount})`)
          }
        }
      }

      const signature = JSON.stringify([
        selectionMode, harvestMode, harvestCategory, harvestQuery, harvestBatchBusy, tvboxConfigured,
        [...harvestCheckedIds], selectionResourceIds?.size, selectionEmptyMessage,
        entries.map(resource => [
          resource.id, resource.url, resource.audioUrl, resource.title, resource.filename, resource.kind,
          resource.size, resource.estimatedSize, resource.width, resource.height, resource.bandwidth,
          resource.quality, resource.duration, resource.isLive, resource.drmProtected, resource.variants,
          preferredVariants.get(resourceFingerprint(resource)),
        ]),
        [...resourceSendStates],
      ])
      // 播放进度和重复网络观察不能不断替换用户正在点击的按钮及清晰度控件。
      if (signature === renderedListSignature) { updateVideoButtons(); return }
      renderedListSignature = signature
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      if (selectionMode && selectionResourceIds?.size === 0 && result) {
        result.hidden = false
        result.classList.remove('info')
        result.classList.add('error')
        result.dataset.source = 'selection'
        setTextIfChanged(result, selectionEmptyMessage)
      } else if (harvestMode && allSelectionEntries.length > 0 && entries.length === 0 && result) {
        result.hidden = false
        result.classList.remove('error')
        result.classList.add('info')
        result.dataset.source = 'selection'
        setTextIfChanged(result, '当前分类或搜索词下没有匹配的链接')
      } else if (result?.dataset.source === 'selection') {
        result.hidden = true
        result.classList.remove('error', 'info')
        delete result.dataset.source
      }
      patchKeyedChildren(
        list,
        entries,
        resource => resourceFingerprint(resource),
        createPanelRow,
        updatePanelRow,
      )
      updateVideoButtons()
      // 聚合 iframe 资源会增加面板高度；在用户移入之前重新定位，避免鼠标下的按钮突然换位。
      if (wrap?.classList.contains('open')) fitPanel()
    }
    let renderFrame = 0
    const scheduleRender = () => {
      if (renderFrame || ctx.isInvalid) return
      renderFrame = ctx.requestAnimationFrame(() => {
        renderFrame = 0
        render()
      })
    }
    let positionFrame = 0
    const scheduleVideoButtons = () => {
      if (positionFrame || ctx.isInvalid) return
      positionFrame = ctx.requestAnimationFrame(() => {
        positionFrame = 0
        updateHoveredPlayer()
        updateVideoButtons()
      })
    }
    let pointer = { x: -1, y: -1, inControls: false }
    const updateHoveredPlayer = () => {
      if (videoControlDragging || pointer.inControls || ui.shadow.activeElement) return
      const candidates = currentVideos().filter(video => {
        if (!playbackByVideo.has(video) && !video.paused && !video.ended) {
          markVideoPlayback(video, 'sync')
        }
        if (!playbackByVideo.has(video)) return false
        const rect = visibleMediaRect(video, { width: innerWidth, height: innerHeight })
        return rect && rect.width >= 180 && rect.height >= 100
          && pointer.x >= rect.left && pointer.x <= rect.right && pointer.y >= rect.top && pointer.y <= rect.bottom
      })
      const target = mediaElementAtPoint(candidates, pointer)
      if (target) {
        if (playerDismissTimer) clearTimeout(playerDismissTimer)
        playerDismissTimer = undefined
        hoveredVideo = target
      } else if (hoveredVideo && !playerDismissTimer) {
        // 留出跨越播放器与按钮间隙的时间，避免鼠标移向按钮时按钮先消失。
        playerDismissTimer = setTimeout(() => {
          playerDismissTimer = undefined
          hoveredVideo = null
          scheduleVideoButtons()
        }, HOVER_DISMISS_DELAY_MS)
      }
    }
    const POINTER_HIT_TEST_INTERVAL_MS = 80
    let lastPointerHitTestAt = 0
    let pointerThrottleTimer: ReturnType<typeof setTimeout> | null = null
    const schedulePointerHitTest = () => {
      if (ctx.isInvalid || videoControlDragging) return
      if (knownVideos.size === 0 && !hoveredVideo) return
      const now = Date.now()
      const elapsed = now - lastPointerHitTestAt
      if (elapsed >= POINTER_HIT_TEST_INTERVAL_MS) {
        if (pointerThrottleTimer) {
          clearTimeout(pointerThrottleTimer)
          pointerThrottleTimer = null
        }
        lastPointerHitTestAt = now
        scheduleVideoButtons()
        return
      }
      if (pointerThrottleTimer) return
      pointerThrottleTimer = setTimeout(() => {
        pointerThrottleTimer = null
        lastPointerHitTestAt = Date.now()
        scheduleVideoButtons()
      }, POINTER_HIT_TEST_INTERVAL_MS - elapsed)
    }
    ctx.addEventListener(window, 'pointermove', event => {
      if (videoControlDragging || (knownVideos.size === 0 && !hoveredVideo)) return
      pointer = { x: event.clientX, y: event.clientY, inControls: event.composedPath().includes(ui.shadow.host) }
      schedulePointerHitTest()
    }, { capture: true, passive: true })
    ctx.addEventListener(document, 'pointerout', event => {
      if (event.relatedTarget) return
      pointer = { x: -1, y: -1, inControls: false }
      scheduleVideoButtons()
    }, { capture: true, passive: true })
    ctx.onInvalidated(() => {
      if (playerDismissTimer) clearTimeout(playerDismissTimer)
      if (pointerThrottleTimer) clearTimeout(pointerThrottleTimer)
    })
    ctx.addEventListener(window, 'scroll', scheduleVideoButtons, { capture: true, passive: true })
    ctx.addEventListener(window, 'resize', scheduleVideoButtons)
    try {
      const parentDocument = window.frameElement?.ownerDocument
      const parentWindow = parentDocument?.defaultView
      if (parentDocument && parentWindow) {
        ctx.addEventListener(parentWindow, 'resize', scheduleVideoButtons)
        ctx.addEventListener(parentWindow, 'scroll', scheduleVideoButtons, { capture: true, passive: true })
        ctx.addEventListener(parentDocument, 'scroll', scheduleVideoButtons, { capture: true, passive: true })
      }
    } catch {
      // Cross-origin parent frames keep the child viewport bounds.
    }
    const syncOverlayHostContainer = () => {
      const fullscreen = document.fullscreenElement
      const targetContainer = fullscreen && fullscreen.tagName !== 'VIDEO'
        ? fullscreen
        : (document.body || document.documentElement)
      if (targetContainer && ui.shadowHost.parentElement !== targetContainer) {
        targetContainer.append(ui.shadowHost)
      }
    }
    ctx.addEventListener(document, 'fullscreenchange', () => {
      syncOverlayHostContainer()
      scheduleVideoButtons()
    })

    videoResizeObserver = typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver(() => scheduleVideoButtons())
    ctx.onInvalidated(() => videoResizeObserver?.disconnect())
    videoIntersectionObserver = typeof IntersectionObserver === 'undefined'
      ? null : new IntersectionObserver(() => scheduleVideoButtons())
    ctx.onInvalidated(() => videoIntersectionObserver?.disconnect())
    let geometryTargets = new Set<HTMLElement>()
    const geometryObserver = new MutationObserver(() => scheduleVideoButtons())
    const syncMediaGeometry = () => {
      const next = new Set(currentVideos().flatMap(video => [...mediaOverlayAncestors(video)]))
      if (next.size === geometryTargets.size && [...next].every(target => geometryTargets.has(target))) return
      // Observe player geometry directly. Chat/animation attributes elsewhere
      // must not produce a document-wide stream of mutation records.
      geometryObserver.disconnect()
      for (const target of geometryTargets) {
        if (!next.has(target)) videoResizeObserver?.unobserve(target)
      }
      for (const target of next) {
        geometryObserver.observe(target, { attributes: true, attributeFilter: ['class', 'style', 'hidden'] })
        if (!geometryTargets.has(target)) videoResizeObserver?.observe(target)
      }
      geometryTargets = next
    }
    ctx.onInvalidated(() => { geometryObserver.disconnect(); geometryTargets.clear() })
    const trackVideo = (video: HTMLVideoElement) => {
      knownVideos.add(video)
      if (observedVideoSizes.has(video)) return
      observedVideoSizes.add(video)
      videoResizeObserver?.observe(video)
      videoIntersectionObserver?.observe(video)
    }

    const explicitElementSource = (element: HTMLMediaElement | HTMLSourceElement): string => {
      // `element.src` resolves an absent/empty attribute to the page URL. Read
      // the content attribute first so an idle <video src=""> cannot turn the
      // current document (for example view_video.php) into a download item.
      const raw = String(element.getAttribute('src') || '').trim()
      if (!raw) return ''
      try { return new URL(raw, document.baseURI).href } catch { return '' }
    }
    const mediaElementSources = (media: HTMLMediaElement): Array<{ url: string, mimeType: string }> => {
      const declared = [
        { url: explicitElementSource(media), mimeType: '' },
        ...[...media.querySelectorAll<HTMLSourceElement>('source')]
          .map(source => ({ url: explicitElementSource(source), mimeType: source.type || '' })),
      ].filter(source => Boolean(source.url) && !isSameDocumentPlaybackFallback(source.url, location.href))
      const current = String(media.currentSrc || '').trim()
      // A blob: currentSrc is legitimate MSE evidence. For http(s), retain the
      // current source only if it is not the browser's empty-src document
      // fallback, or a watch-page URL written into src="" / src="this page".
      const currentIsDocumentFallback = /^https?:\/\//i.test(current)
        && isSameDocumentPlaybackFallback(current, location.href)
      // 多个 <source> 是格式候选；浏览器选定后仅当前资源属于这个播放实例。
      if (/^https?:\/\//i.test(current) && !currentIsDocumentFallback && media.readyState >= 1) {
        return [{ url: current, mimeType: declared.find(source => source.url === current)?.mimeType || '' }]
      }
      return [
        ...(current && !currentIsDocumentFallback ? [{ url: current, mimeType: '' }] : []),
        ...declared,
      ]
    }
    const add = (
      url: string,
      mimeType = '',
      playbackSource = false,
      evidence: string[] = [],
      owner = '',
      confidence = 0,
      inspection?: Partial<MediaResource>,
      observedIdentity?: Record<string, string>,
      audioUrl?: string,
    ) => {
      const confirmedFullTrack = evidence.includes('site_player_metadata') || evidence.includes('mse_full_track')
      const kind = playbackSource
        ? classifyPlaybackSource(url, mimeType)
        : (classifyResource(url, mimeType)
          || (confirmedFullTrack && /^https?:\/\//i.test(url)
            && (mimeType.toLowerCase().startsWith('video/') || isBilibiliFullTrackMseUrl(url))
            ? 'media'
            : null))
      if (!kind) return
      const httpUrlOnly = (value: unknown): string | undefined => {
        if (typeof value !== 'string' || !value.trim()) return undefined
        try {
          const parsed = new URL(value)
          return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.href : undefined
        } catch {
          return undefined
        }
      }
      const safeAudioUrl = httpUrlOnly(audioUrl || inspection?.audioUrl)
      let filename = typeof inspection?.filename === 'string' && inspection.filename.trim()
        ? inspection.filename.trim()
        : ''
      if (!filename) {
        try { filename = decodeURIComponent(new URL(url).pathname.split('/').pop() || '') } catch {}
      }
      if ((safeAudioUrl || isBilibiliFullTrackMseUrl(url)) && /\.m4s$/i.test(filename)) {
        filename = filename.replace(/\.m4s$/i, '.mp4')
      }
      const safeVariants = Array.isArray(inspection?.variants)
        ? inspection.variants
            .map(variant => {
              if (!variant || typeof variant !== 'object' || Array.isArray(variant)) return null
              const safeUrl = httpUrlOnly(variant.url)
              if (!safeUrl) return null
              const variantAudioUrl = httpUrlOnly(variant.audioUrl) || safeAudioUrl
              const safeVariantId = typeof variant.variantId === 'string' && variant.variantId.trim()
                ? variant.variantId.trim().slice(0, 256)
                : undefined
              return {
                url: safeUrl,
                ...(variantAudioUrl ? { audioUrl: variantAudioUrl } : {}),
                ...(safeVariantId ? { variantId: safeVariantId } : {}),
                ...(typeof variant.width === 'number' && Number.isFinite(variant.width) && variant.width > 0 ? { width: variant.width } : {}),
                ...(typeof variant.height === 'number' && Number.isFinite(variant.height) && variant.height > 0 ? { height: variant.height } : {}),
                ...(typeof variant.bandwidth === 'number' && Number.isFinite(variant.bandwidth) && variant.bandwidth > 0 ? { bandwidth: variant.bandwidth } : {}),
                ...(typeof variant.quality === 'string' && variant.quality.trim() ? { quality: variant.quality.trim().slice(0, 64) } : {}),
              }
            })
            .filter((variant): variant is NonNullable<typeof variant> => variant !== null)
            .slice(0, 12)
        : undefined
      const safeAudioTracks = Array.isArray(inspection?.audioTracks)
        ? inspection.audioTracks
            .map(track => {
              if (!track || typeof track !== 'object' || Array.isArray(track)) return null
              const safeUrl = track.url ? httpUrlOnly(track.url) : undefined
              if (track.url && !safeUrl) return null
              return {
                label: typeof track.label === 'string' ? track.label.slice(0, 128) : '',
                language: typeof track.language === 'string' ? track.language.slice(0, 64) : '',
                ...(safeUrl ? { url: safeUrl } : {}),
                ...(typeof track.default === 'boolean' ? { default: track.default } : {}),
              }
            })
            .filter((track): track is NonNullable<typeof track> => track !== null)
            .slice(0, 24)
        : undefined
      const safeUrlList = (values: unknown, limit: number): string[] | undefined => (
        Array.isArray(values)
          ? values.map(httpUrlOnly).filter((item): item is string => Boolean(item)).slice(0, limit)
          : undefined
      )
      const safeHashList = (values: unknown, limit: number): string[] | undefined => (
        Array.isArray(values)
          ? values
              .filter((item): item is string => typeof item === 'string' && item.length > 0 && item.length <= 128)
              .slice(0, limit)
          : undefined
      )
      const resource = {
        // MAIN 桥仅接收解析结果，不能用网页事件覆盖请求身份或凭据，且只接受 http(s) 地址。
        ...(inspection?.inspected === true ? {
          inspected: true, manifestType: inspection.manifestType, isLive: inspection.isLive,
          lowLatencyLive: inspection.lowLatencyLive, partOnlyLive: inspection.partOnlyLive,
          ...(inspection.drmProtected === true ? { drmProtected: true } : {}),
          duration: inspection.duration, variants: safeVariants, audioTracks: safeAudioTracks,
          renditionUrls: safeUrlList(inspection.renditionUrls, 24),
          playbackUrls: safeUrlList(inspection.playbackUrls, 48),
          playbackUrlHashes: safeHashList(inspection.playbackUrlHashes, 512),
          playbackPatterns: safeUrlList(inspection.playbackPatterns, 48),
          width: inspection.width, height: inspection.height, bandwidth: inspection.bandwidth,
          estimatedSize: inspection.estimatedSize, quality: inspection.quality,
        } : {}),
        ...(safeAudioUrl ? { audioUrl: safeAudioUrl } : {}),
        id: resourceId(url), url, kind, mimeType, pageUrl: location.href,
        title: pageMediaTitle() || filename, filename, seenAt: Date.now(),
        evidence: evidence.length ? evidence.slice(0, 16) : [playbackSource ? 'current_src' : 'page_observation'],
        owner: owner || (playbackSource ? 'media-element' : 'page'),
        confidence: boundedConfidence(confidence || (playbackSource ? 0.98 : 0.58)),
        replayContext: { method: 'GET', page_url: location.href.split('#', 1)[0] },
        ...(observedIdentity ? { responseIdentity: observedIdentity } : {}),
      }
      const changed = addResource(resource)
      if (changed) {
        confirmAllKnownVideoPlaybacks()
        scheduleRender()
      }
      const write = runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
      pendingResourceWrites.add(write)
      void write.finally(() => pendingResourceWrites.delete(write))
      return write
    }

    const markAudioPlayback = (audio: HTMLAudioElement) => {
      if (ctx.isInvalid || !audio.isConnected) return
      if (audio.paused && audio.readyState < 1 && !audio.currentSrc) return
      const directSources = mediaElementSources(audio)
      const seenDirectSources = new Set<string>()
      directSources.forEach(source => {
        if (!source.url || seenDirectSources.has(source.url)) return
        seenDirectSources.add(source.url)
        add(source.url, source.mimeType || 'audio/mpeg', true, ['current_src', 'playback_confirmed'], mediaOwner(audio), 0.98)
      })
    }

    const markVideoPlayback = (video: HTMLVideoElement, eventType: string) => {
      if (ctx.isInvalid) return
      const rect = video.getBoundingClientRect()
      const tinyDecorative = rect.width > 0 && rect.height > 0 && (rect.width < 64 || rect.height < 48)
      if (tinyDecorative) return
      const visibleInViewport = rect.width >= 180
        && rect.height >= 100
        && rect.bottom >= 0
        && rect.top <= innerHeight
        && rect.right >= 0
        && rect.left <= innerWidth
      const hasMediaEvidence = !video.paused || video.readyState >= 1 || Boolean(video.currentSrc)
      if (!visibleInViewport && !hasMediaEvidence) return
      const previousActiveVideo = activeVideo
      const explicitActivation = ['play', 'playing', 'initializing'].includes(eventType)
      // timeupdate fires several times per second for every playing element.
      // It may refresh source evidence, but must not continuously steal the
      // active-player identity or rebuild the complete overlay DOM.
      if (visibleInViewport && (!video.paused || explicitActivation)
        && (explicitActivation || !activeVideo || !activeVideo.isConnected || activeVideo.paused || activeVideo.ended)) {
        const changedPlayer = activeVideo !== video
        activeVideo = video
        if (changedPlayer && explicitActivation) {
          selectionMode = false
          harvestMode = false
          selectionResourceIds = null
        }
      }
      const directSources = mediaElementSources(video)
      const sourceUrls = [...new Set(directSources.map(source => source.url).filter(Boolean))]
      const mseResourceUrls = sourceUrls
        .filter(source => source.startsWith('blob:'))
        .flatMap(source => [...(mseEvidenceByBlob.get(source)?.urls || [])])
      const previousPlayback = playbackByVideo.get(video) || null
      const changedSource = sourceUrls.join('\n') !== (previousPlayback?.sourceUrls || []).join('\n')
      if (!previousPlayback || changedSource) {
        if (changedSource) videoButtonPositions.delete(video)
        const playback = { sourceUrls, startedAt: Date.now(), mseResourceUrls }
        playbackByVideo.set(video, playback)
        if (video === activeVideo) activePlayback = playback
        // currentSrc is immediate, player-specific evidence. Register it here
        // instead of waiting for PerformanceObserver/webRequest: Firefox can
        // omit those observations for dynamically created iframe media.
        const seenDirectSources = new Set<string>()
        const sourceEvidence = hasMediaEvidence ? ['current_src', 'playback_confirmed'] : ['current_src']
        directSources.forEach(source => {
          if (!source.url || seenDirectSources.has(source.url)) return
          seenDirectSources.add(source.url)
          add(source.url, source.mimeType, true, sourceEvidence, mediaOwner(video), 0.98)
        })
        confirmPlaybackChoices(playback)
      } else {
        if (video === activeVideo) activePlayback = previousPlayback
        confirmPlaybackChoices(previousPlayback)
        if (previousActiveVideo !== activeVideo) scheduleVideoButtons()
        return
      }
      render()
    }
    const markPlayback = (event: Event) => {
      const video = eventVideo(event)
      if (video) {
        if (
          video.paused
          && video.readyState < 1
          && !video.currentSrc
          && !playbackByVideo.has(video)
          && ['loadedmetadata', 'loadeddata', 'timeupdate'].includes(event.type)
        ) return
        markVideoPlayback(video, event.type)
        return
      }
      const audio = eventAudio(event)
      if (audio) markAudioPlayback(audio)
    }
    const syncPlayingVideos = () => {
      currentVideos().forEach(video => {
        if (!video.paused && !video.ended) markVideoPlayback(video, 'sync')
        else if (video.readyState >= 1 || Boolean(video.currentSrc)) markVideoPlayback(video, 'loadedmetadata')
      })
      for (const audio of knownAudios) {
        if (!audio.isConnected) {
          knownAudios.delete(audio)
          continue
        }
        if (!audio.paused || audio.readyState >= 1 || Boolean(audio.currentSrc)) {
          markAudioPlayback(audio)
        }
      }
    }
    const observedMediaRoots = new WeakSet<Document | ShadowRoot>()
    const knownShadowRoots = new Set<ShadowRoot>()
    const mediaEvents = ['play', 'playing', 'loadedmetadata', 'loadeddata', 'timeupdate'] as const
    const trackAudio = (audio: HTMLAudioElement) => {
      knownAudios.add(audio)
    }
    const registerMediaRoot = (root: Document | ShadowRoot) => {
      if (observedMediaRoots.has(root)) return
      observedMediaRoots.add(root)
      if (root instanceof ShadowRoot) knownShadowRoots.add(root)
      const discover = (node: ParentNode | Element) => {
        discoverSubtitlePlayers(node)
        if (node instanceof HTMLVideoElement) trackVideo(node)
        if (node instanceof HTMLAudioElement) trackAudio(node)
        node.querySelectorAll<HTMLVideoElement>('video').forEach(trackVideo)
        node.querySelectorAll<HTMLAudioElement>('audio').forEach(trackAudio)
        const inspect = (element: Element) => {
          if (element.shadowRoot) registerMediaRoot(element.shadowRoot)
        }
        if (node instanceof Element) inspect(node)
        // Existing shadow players are worth a bounded scan; an unbounded
        // `querySelectorAll('*')` makes live-chat DOM churn delay the very
        // overlay the user is waiting for. New roots are signalled directly
        // by the MAIN-world attachShadow hook below.
        const walker = document.createTreeWalker(node, NodeFilter.SHOW_ELEMENT)
        let visited = 0
        let current = walker.nextNode() as Element | null
        while (current && visited < 4096) {
          inspect(current)
          visited += 1
          current = walker.nextNode() as Element | null
        }
      }
      discover(root)
      syncMediaGeometry()
      mediaEvents.forEach(type => ctx.addEventListener(root, type, markPlayback, { capture: true }))
      ctx.addEventListener(root, 'slotchange', () => {
        syncMediaGeometry()
        scheduleVideoButtons()
      }, { capture: true })
      const mediaObserver = new MutationObserver(mutations => {
        // 播放器被移除时也要撤掉旧入口，并释放 ResizeObserver 对元素的引用。
        let playbackChanged = mutations.some(mutation => mutation.removedNodes.length > 0)
          && [...knownVideos].some(video => !video.isConnected)
        mutations.forEach(mutation => {
          if (mutation.type === 'childList') {
            mutation.addedNodes.forEach(node => {
              if (!(node instanceof Element)) return
              const hasShadowCandidate = Boolean(node.shadowRoot) || node.tagName.includes('-')
              const containsMedia = hasShadowCandidate
                || node.matches('video,audio,source')
                || Boolean(node.querySelector('video,audio,source'))
                || [...geometryTargets].some(target => node.contains(target))
              if (!containsMedia) return
              discover(node)
              playbackChanged = true
            })
          } else if (
            mutation.attributeName === 'src'
            && mutation.target instanceof Element
            && mutation.target.matches('video,audio,source')
          ) {
            playbackChanged = true
          }
        })
        if (playbackChanged) {
          syncMediaGeometry()
          scheduleVideoButtons()
          queueMicrotask(syncPlayingVideos)
        }
      })
      mediaObserver.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] })
      ctx.onInvalidated(() => mediaObserver.disconnect())
    }
    ctx.addEventListener(window, '__hls_downloader_shadow__', event => {
      const host = event.composedPath().find(value => value instanceof Element) as Element | undefined
      if (host?.shadowRoot) {
        registerMediaRoot(host.shadowRoot)
        queueMicrotask(syncPlayingVideos)
      }
    }, { capture: true })
    registerMediaRoot(document)
    window.removeEventListener('__hls_downloader_shadow__', earlyShadowListener, true)
    activationMediaRoots.forEach(root => {
      root.removeEventListener('play', rememberPendingPlayback, true)
      root.removeEventListener('playing', rememberPendingPlayback, true)
    })
    activationMediaRoots.length = 0
    pendingPlaybackVideos.forEach(video => markVideoPlayback(video, 'initializing'))
    pendingPlaybackVideos.clear()
    pendingPlaybackAudios.forEach(audio => markAudioPlayback(audio))
    pendingPlaybackAudios.clear()
    const applyStoredOverlayPrefs = (values: Record<string, unknown>) => {
      if ('excludedHosts' in values) excludedSiteHosts = normalizeHostList(values.excludedHosts)
      if ('suppressedSiteHints' in values) suppressedHintHosts = normalizeHostList(values.suppressedSiteHints)
      if ('tvboxConfigured' in values) {
        tvboxConfigured = typeof values.tvboxConfigured === 'boolean' ? values.tvboxConfigured : undefined
      }
      if (isCurrentSiteExcluded() && !selectionMode && !harvestMode) {
        if (pinned) setPinned(false)
        setOpen(false)
        panelPosition = null
      }
      renderedListSignature = ''
      renderedVideoSignature = ''
      scheduleRender()
      scheduleVideoButtons()
    }
    void browser.storage.local.get(['excludedHosts', 'suppressedSiteHints', 'tvboxConfigured'])
      .then(applyStoredOverlayPrefs)
      .catch(() => undefined)
    const onOverlayStorageChanged = (changes: Record<string, { newValue?: unknown }>, area: string) => {
      if (area !== 'local' || ctx.isInvalid) return
      const patch: Record<string, unknown> = {}
      if ('excludedHosts' in changes) patch.excludedHosts = changes.excludedHosts?.newValue
      if ('suppressedSiteHints' in changes) patch.suppressedSiteHints = changes.suppressedSiteHints?.newValue
      if ('tvboxConfigured' in changes) patch.tvboxConfigured = changes.tvboxConfigured?.newValue
      if (Object.keys(patch).length) applyStoredOverlayPrefs(patch)
    }
    browser.storage.onChanged.addListener(onOverlayStorageChanged)
    ctx.onInvalidated(() => browser.storage.onChanged.removeListener(onOverlayStorageChanged))
    syncPlayingVideos()
    document.documentElement.setAttribute('data-hls-downloader-extension', '1')

    const handleResourceEvent = (event: Event) => {
      const detail = bridgeDetail(event)
      if (detail.sourcePageUrl && !samePageOrVideo(detail.sourcePageUrl, location.href)) return
      // A buffered player response may arrive after an SPA navigation.
      if (detail.siteVideoId && detail.siteVideoId !== sitePageVideoId(location.href)) return
      const evidence = detail.siteVideoId
        ? ['site_player_metadata', 'page_response']
        : detail.inspection?.inspected ? ['manifest_inspection', 'page_response'] : []
      add(detail.url, detail.mimeType, false, evidence, '', 0, detail.inspection, detail.responseIdentity, detail.audioUrl)
    }
    const handleMseEvent = (event: Event) => {
      const detail = bridgeDetail(event)
      if (detail.sourcePageUrl && !samePageOrVideo(detail.sourcePageUrl, location.href)) return
      const blobUrl = String(detail.blobUrl || '')
      const mediaUrl = String(detail.mediaUrl || '')
      if (!blobUrl.startsWith('blob:') || !/^https?:/i.test(mediaUrl)) return
      if (detail.purpose === 'download') {
        // Keep this page-scoped correlation in extension memory only. It lets
        // downloads.onCreated replace an opaque blob: item with the successful
        // HTTP response that produced it, without guessing for generated data.
        void runtimeMessage({
          type: 'blob-source',
          blobUrl,
          sourceUrl: mediaUrl,
          pageUrl: location.href,
        }).catch(() => undefined)
      }
      const now = Date.now()
      const evidence = mseEvidenceByBlob.get(blobUrl) || { urls: new Set<string>(), seenAt: now }
      const isNewEvidence = !evidence.urls.has(mediaUrl)
      evidence.urls.add(mediaUrl)
      while (evidence.urls.size > 96) {
        const oldest = evidence.urls.values().next().value
        if (!oldest) break
        evidence.urls.delete(oldest)
      }
      evidence.seenAt = now
      // Refresh insertion order so the bounded map behaves like a small LRU.
      mseEvidenceByBlob.delete(blobUrl)
      mseEvidenceByBlob.set(blobUrl, evidence)
      for (const video of currentVideos()) {
        const playback = playbackByVideo.get(video)
        if (!playback?.sourceUrls.includes(blobUrl)) continue
        const updated = {
          ...playback,
          mseResourceUrls: [...evidence.urls],
        }
        playbackByVideo.set(video, updated)
        if (video === activeVideo) activePlayback = updated
      }
      for (const [key, value] of mseEvidenceByBlob) {
        if (now - value.seenAt > 30 * 60_000) mseEvidenceByBlob.delete(key)
      }
      while (mseEvidenceByBlob.size > 200) {
        const oldest = mseEvidenceByBlob.keys().next().value
        if (!oldest) break
        mseEvidenceByBlob.delete(oldest)
      }
      if (isNewEvidence) {
        // YouTube-style `/videoplayback` often reaches MSE without a manifest.
        // Register that stream as the current player resource; ordinary `.m4s`/`.ts`
        // fragments stay out of the overlay, while Bilibili's full byte-ranged
        // video+audio `.m4s` pair falls back cleanly when playinfo was unavailable.
        const ownerVideo = currentVideos().find(video =>
          mediaElementSources(video).some(source => source.url === blobUrl),
        )
        const owner = ownerVideo ? mediaOwner(ownerVideo) : `mse-source-buffer:${resourceId(blobUrl)}`
        const isCoveredBySiteMetadata = (candidateUrl: string) => [...resources.values()].some(item =>
          item.evidence?.includes('site_player_metadata')
          && (item.url === candidateUrl || (item.playbackUrls || []).includes(candidateUrl)),
        )
        if (!isCoveredBySiteMetadata(mediaUrl) && isBilibiliFullTrackMseUrl(mediaUrl)) {
          const pairedAudioUrl = [...evidence.urls].find(u => isBilibiliAudioTrackMseUrl(u))
          add(mediaUrl, 'video/mp4', false, ['mse_source_buffer', 'mse_full_track'], owner, 0.97, undefined, undefined, pairedAudioUrl)
        } else if (isBilibiliAudioTrackMseUrl(mediaUrl)) {
          for (const candidateUrl of evidence.urls) {
            if (isBilibiliFullTrackMseUrl(candidateUrl) && !isCoveredBySiteMetadata(candidateUrl)) {
              add(candidateUrl, 'video/mp4', false, ['mse_source_buffer', 'mse_full_track'], owner, 0.97, undefined, undefined, mediaUrl)
            }
          }
        } else if (!isMseMediaSliceUrl(mediaUrl)) {
          add(mediaUrl, '', false, ['mse_source_buffer'], owner, 0.97)
        }
        confirmAllKnownVideoPlaybacks()
        scheduleRender()
      }
    }
    window.removeEventListener('__hls_downloader_resource__', earlyResourceListener)
    window.removeEventListener('__hls_downloader_mse__', earlyMseListener)
    ctx.addEventListener(window, '__hls_downloader_resource__', handleResourceEvent)
    ctx.addEventListener(window, '__hls_downloader_mse__', handleMseEvent)
    // Ask the MAIN-world hook to replay events emitted before this content
    // script/UI finished mounting (especially fast MSE and preloaded HLS).
    window.dispatchEvent(new Event('__hls_downloader_replay__'))
    earlyResourceEvents.splice(0).forEach(event => handleResourceEvent(new CustomEvent('__hls_downloader_resource__', { detail: JSON.stringify(event) })))
    earlyMseEvents.splice(0).forEach(event => handleMseEvent(new CustomEvent('__hls_downloader_mse__', { detail: JSON.stringify(event) })))
    document.querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video,audio').forEach(media => {
      const confirmed = !media.paused || media.readyState >= 1 || Boolean(media.currentSrc)
      const ev = confirmed ? ['current_src', 'playback_confirmed'] : ['current_src']
      mediaElementSources(media).forEach(source => add(source.url, source.mimeType || (media instanceof HTMLAudioElement ? 'audio/mpeg' : ''), true, ev, mediaOwner(media), 0.98))
    })
    document.querySelectorAll<HTMLSourceElement>('source').forEach(source => {
      const url = explicitElementSource(source)
      if (url) add(url, source.type || '', true)
    })
    const resourceObserver = new PerformanceObserver(list => list.getEntries().forEach(entry => add(entry.name)))
    resourceObserver.observe({ type: 'resource', buffered: true })
    ctx.onInvalidated(() => resourceObserver.disconnect())
    const handleRuntimeMessage = (message: any, _sender?: unknown, sendResponse?: (value?: unknown) => void) => {
      if (message?.type === 'download-error') {
        selectionMode = true
        harvestMode = false
        selectionResourceIds = null
        openFloatingPanel()
        const result = ui.shadow.querySelector<HTMLElement>('.result')
        if (result) {
          result.hidden = false
          result.classList.remove('info')
          result.classList.add('error')
          result.dataset.source = 'download'
          result.textContent = String(message.error || '下载请求失败，请重试')
        }
        sendResponse?.({ ok: true })
        return
      }
      if (message?.type === 'captured-resource' && message.resource?.url) {
        const pageTitle = pageMediaTitle()
        const resource = {
          ...message.resource,
          title: !message.resource.title || isGenericMediaName(message.resource.title) ? pageTitle || message.resource.title : message.resource.title,
          id: resourceId(message.resource.url),
          seenAt: Date.now(),
        } as MediaResource
        if (addResource(resource)) scheduleRender()
        // The background already persisted captured-resource. Send a second
        // update only when this frame contributes a better human title.
        if (resource.title && resource.title !== message.resource.title) {
          void runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
        }
        return
      }
      if (message?.type === 'collect-selection') {
        const selection = window.getSelection()
        if (!selection?.rangeCount) {
          sendResponse?.({ ok: true, count: 0 })
          return
        }
        const root = selection.getRangeAt(0).cloneContents()
        const urls = selectedDownloadUrls(
          [...root.querySelectorAll<HTMLAnchorElement>('a[href]')].map(anchor => anchor.getAttribute('href') || ''),
          selection.toString(),
          document.baseURI || location.href,
        )
        selectionEmptyMessage = '选中的内容里没有可下载链接'
        selectionMode = true
        harvestMode = false
        selectionResourceIds = new Set<string>()
        activePlayback = null
        activeVideo = null
        urls.forEach(url => {
          const kind = classifyResource(url) || (/^(?:https?|magnet):/i.test(url) ? 'file' : null)
          if (!kind) return
          let filename = ''
          try { filename = decodeURIComponent(new URL(url).pathname.split('/').pop() || '') } catch {}
          const resource: MediaResource = {
            id: resourceId(url), url, kind, pageUrl: location.href, title: filename || url,
            filename, seenAt: Date.now(), evidence: ['text_selection'], owner: 'selection', confidence: 0.99,
          }
          addResource(resource)
          selectionResourceIds?.add(resourceFingerprint(resource))
        })
        setPanelTitle('选中的链接')
        openFloatingPanel()
        sendResponse?.({ ok: true, count: selectionResourceIds.size })
        return
      }
      if (message?.type === 'collect-page-links') {
        for (const shadowRoot of knownShadowRoots) {
          if (!shadowRoot.host.isConnected) knownShadowRoots.delete(shadowRoot)
        }
        const queryRoots: Array<Document | ShadowRoot> = [document, ...knownShadowRoots]
        const anchorCandidates = queryRoots.flatMap(root =>
          [...root.querySelectorAll<HTMLAnchorElement | HTMLAreaElement>('a[href], area[href]')]
            .map(anchor => ({
              href: anchor.getAttribute('href') || '',
              download: anchor.hasAttribute('download'),
            })),
        )
        const dataDownloadCandidates = queryRoots.flatMap(root =>
          [...root.querySelectorAll<HTMLElement>('[data-download-url], [data-file-url], [data-export-url]')]
            .map(element => ({
              href: element.getAttribute('data-download-url')
                || element.getAttribute('data-file-url')
                || element.getAttribute('data-export-url')
                || '',
              download: true,
            })),
        )
        const mediaSrcs = [
          ...queryRoots.flatMap(root => [...root.querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video, audio')])
            .flatMap(media => mediaElementSources(media).map(source => source.url)),
          ...queryRoots.flatMap(root => [...root.querySelectorAll<HTMLSourceElement>('source')])
            .map(source => explicitElementSource(source)),
        ].filter(Boolean)
        const urls = pageDownloadUrls(
          [...anchorCandidates, ...dataDownloadCandidates],
          mediaSrcs,
          document.baseURI || location.href,
          200,
        )
        selectionEmptyMessage = '当前页面没有发现可下载链接'
        selectionMode = true
        harvestMode = true
        harvestCategory = 'all'
        harvestQuery = ''
        harvestCheckedIds.clear()
        if (harvestSearchEl) harvestSearchEl.value = ''
        const result = ui.shadow.querySelector<HTMLElement>('.result')
        if (result?.dataset.source === 'batch') {
          result.hidden = true
          result.classList.remove('error', 'info')
          delete result.dataset.source
        }
        selectionResourceIds = new Set<string>()
        activePlayback = null
        activeVideo = null
        for (const existing of resources.values()) {
          if (['hls', 'dash', 'media'].includes(existing.kind)) {
            selectionResourceIds.add(resourceFingerprint(existing))
          }
        }
        urls.forEach(url => {
          const kind = classifyResource(url) || (/^(?:https?|magnet):/i.test(url) ? 'file' : null)
          if (!kind) return
          let filename = ''
          try { filename = decodeURIComponent(new URL(url).pathname.split('/').pop() || '') } catch {}
          const resource: MediaResource = {
            id: resourceId(url), url, kind, pageUrl: location.href, title: filename || url,
            filename, seenAt: Date.now(), evidence: ['page_links_harvest'], owner: 'page-harvest', confidence: 0.95,
          }
          addResource(resource)
          selectionResourceIds?.add(resourceFingerprint(resource))
        })
        setPanelTitle('页面可下载链接')
        openFloatingPanel()
        sendResponse?.({ ok: true, count: selectionResourceIds.size })
        return
      }
      if (message?.type === 'open-media-panel') {
        harvestMode = false
        if (message.scope === 'tab') {
          activePlayback = null
          activeVideo = null
          selectionMode = true
          loadPageResources(location.href, 'tab')
        } else if (!activePlayback || !activeVideo || !visibleMediaRect(activeVideo, { width: innerWidth, height: innerHeight })) {
          selectionMode = true
        }
        selectionResourceIds = null
        setPanelTitle('当前视频')
        openFloatingPanel()
        sendResponse?.({ ok: true })
        return
      }
      if (message?.type === 'rescan-media') {
        // A popup-initiated rescan must collect fresh page evidence without
        // forcing the larger in-page panel open. Replay early MAIN-world MSE
        // observations, re-read current media elements (including shadow roots), then persist/render.
        window.dispatchEvent(new Event('__hls_downloader_replay__'))
        const scannedMedia = new Set<HTMLMediaElement>([
          ...document.querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video,audio'),
          ...currentVideos(),
        ])
        scannedMedia.forEach(media => {
          mediaElementSources(media).forEach(source => add(source.url, source.mimeType, true, ['manual_rescan'], mediaOwner(media), 0.98))
        })
        document.querySelectorAll<HTMLSourceElement>('source').forEach(source => {
          const url = explicitElementSource(source)
          if (url) add(url, source.type || '', true, ['manual_rescan'])
        })
        performance.getEntriesByType('resource').forEach(entry => add(entry.name))
        syncPlayingVideos()
        scheduleRender()
        void Promise.all([...pendingResourceWrites]).then(
          () => sendResponse?.({ ok: true }),
          () => sendResponse?.({ ok: false }),
        )
        return true
      }
    }
    browser.runtime.onMessage.addListener(handleRuntimeMessage)
    ctx.onInvalidated(() => browser.runtime.onMessage.removeListener(handleRuntimeMessage))
    contentReady = true
    browser.runtime.onMessage.removeListener(earlyRuntimeListener)
    earlyRuntimeMessages.splice(0).forEach(entry => handleRuntimeMessage(entry.message, undefined, entry.sendResponse))
    let currentPageUrl = pageKey(location.href)
    function loadPageResources(pageUrl: string, scope?: 'tab') {
      void runtimeMessage({ type: 'list', pageUrl, ...(scope ? { scope } : {}) }).then((stored: MediaResource[]) => {
        if (ctx.isInvalid || !Array.isArray(stored) || !samePageOrVideo(location.href, pageUrl)) return
        stored.forEach(resource => {
          if (resource?.url) addResource({
            ...resource,
            title: !resource.title || isGenericMediaName(resource.title) ? pageMediaTitle() || resource.title : resource.title,
          })
        })
        render()
      }).catch(() => undefined)
    }
    const syncPage = () => {
      const next = pageKey(location.href)
      if (next === currentPageUrl) return
      const previousPage = currentPageUrl
      const previousSiteVideoId = sitePageVideoId(currentPageUrl)
      const nextSiteVideoId = sitePageVideoId(next)
      currentPageUrl = next
      // Locale/tracking replaceState redirects (e.g. Bilibili /video/BV... -> /en/video/BV...)
      // keep the same logical video playing; preserve its MSE provenance and captured streams.
      if (previousSiteVideoId && previousSiteVideoId === nextSiteVideoId) {
        if (sameSiteVideoIdentity(previousPage, next)) {
          loadPageResources(location.href)
          window.dispatchEvent(new Event('__hls_downloader_replay__'))
          syncPlayingVideos()
          scheduleVideoButtons()
          return
        }
      }
      const activeBlobs = new Set(
        currentVideos().flatMap(video => mediaElementSources(video).map(source => source.url).filter(url => url.startsWith('blob:'))),
      )
      activePlayback = null
      activeVideo = null
      selectionMode = false
      harvestMode = false
      harvestCheckedIds.clear()
      selectionResourceIds = null
      playbackByVideo = new WeakMap<HTMLVideoElement, PlaybackContext>()
      for (const blobUrl of [...mseEvidenceByBlob.keys()]) {
        if (!activeBlobs.has(blobUrl)) mseEvidenceByBlob.delete(blobUrl)
      }
      videoButtonPositions = new WeakMap<HTMLVideoElement, { x: number, y: number }>()
      panelPosition = null
      if (pinned) setPinned(false)
      setOpen(false)
      resources.clear(); render(); loadPageResources(location.href)
      window.dispatchEvent(new Event('__hls_downloader_replay__'))
      document.querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video,audio').forEach(media => {
        mediaElementSources(media).forEach(source => add(source.url, source.mimeType, true))
      })
      document.querySelectorAll<HTMLSourceElement>('source').forEach(source => {
        const url = explicitElementSource(source)
        if (url) add(url, source.type || '', true)
      })
      syncPlayingVideos()
    }
    loadPageResources(location.href)
    ctx.addEventListener(window, 'popstate', syncPage)
    ctx.addEventListener(window, 'hashchange', syncPage)
    ctx.addEventListener(window, '__hls_downloader_navigation__', syncPage)
    ctx.addEventListener(window, 'pageshow', () => { syncPage(); syncPlayingVideos() })
    ctx.addEventListener(document, 'visibilitychange', () => {
      scheduleVideoButtons()
      if (document.visibilityState === 'visible') {
        syncPage()
        syncPlayingVideos()
      }
    })
  },
})

function pageKey(value: string): string {
  try { const url = new URL(value); url.hash = ''; return url.href } catch { return value.split('#', 1)[0] }
}
