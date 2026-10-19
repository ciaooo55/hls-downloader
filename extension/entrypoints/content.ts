import { browser } from 'wxt/browser'
import { HOVER_DISMISS_DELAY_MS, clampOverlayPosition, overlayActionFallback, overlayResourceDetails, overlaySendKey, safeResourceLocation, shouldKeepHoverOpen, shouldShowMediaOverlay, type OverlayAction } from '../lib/mediaOverlay'
import { boundedConfidence, classifyPlaybackSource, classifyResource, compactResources, isGenericMediaName, isSameDocumentPlaybackFallback, mergeResources, playerPlaybackResources, resourceFingerprint, resourceId, resourceMatchesPlaybackSource, resourceRank, selectMediaVariant, visiblePlaybackResources, type MediaResource, type PlaybackContext } from '../lib/resources'
import { resourceQuality } from '../lib/hlsManifest'
import { THEME_BASE_CSS, THEME_STORAGE_KEY, THEME_TOKENS_CSS, applyTheme, normalizeThemePreference } from '../lib/theme'
import { withDeadline } from '../lib/asyncDeadline'
import { waitForDownloadConfirmation } from '../lib/downloadConfirmation'
import { mediaPushTerminalResult } from '../lib/mediaPush'
import { selectedDownloadUrls } from '../lib/selectionLinks'
import { formatBytes, formatDuration } from '../lib/format'
import { installSubtitleContent } from '../lib/subtitleContent'

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
    const rememberPendingPlayback = (event: Event) => {
      const video = eventVideo(event)
      if (video) {
        pendingPlaybackVideos.add(video)
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
    const earlyResourceEvents: Array<{ url: string; mimeType?: string; inspection?: Partial<MediaResource> }> = []
    const earlyMseEvents: Array<{ blobUrl: string; mediaUrl: string; purpose?: 'mse' | 'download' }> = []
    const earlyRuntimeMessages: any[] = []
    let contentReady = false
    const earlyResourceListener = (event: Event) => {
      if (contentReady) return
      const detail = bridgeDetail(event)
      if (typeof detail.url === 'string') {
        earlyResourceEvents.push({ url: detail.url, mimeType: detail.mimeType, inspection: detail.inspection })
        if (classifyResource(detail.url, detail.mimeType)) requestActivation()
      }
      if (earlyResourceEvents.length > 200) earlyResourceEvents.splice(0, earlyResourceEvents.length - 200)
    }
    const earlyMseListener = (event: Event) => {
      if (contentReady) return
      const detail = bridgeDetail(event)
      if (typeof detail.blobUrl === 'string' && typeof detail.mediaUrl === 'string') {
        earlyMseEvents.push({
          blobUrl: detail.blobUrl,
          mediaUrl: detail.mediaUrl,
          purpose: detail.purpose === 'download' ? 'download' : 'mse',
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
    const earlyRuntimeListener = (message: any) => {
      if (contentReady || !['captured-resource', 'collect-selection', 'open-media-panel', 'download-error'].includes(String(message?.type || ''))) return
      if (message.type === 'captured-resource' && !message.resource?.url) return
      earlyRuntimeMessages.push(message)
      if (earlyRuntimeMessages.length > 100) earlyRuntimeMessages.splice(0, earlyRuntimeMessages.length - 100)
      requestActivation()
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
      while (current && visited < 512) {
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
    const resourceSendStates = new Map<string, { label: string, disabled: boolean, error?: string }>()
    const preferredVariants = new Map<string, { height: number, bandwidth: number }>()
    let renderedListSignature = ''
    let renderedVideoSignature = ''
    let activePlayback: PlaybackContext | null = null
    let activeVideo: HTMLVideoElement | null = null
    let selectionMode = false
    let selectionResourceIds: Set<string> | null = null
    // A page can contain several players.  Keep playback evidence per element
    // instead of treating the latest network manifest as belonging to every
    // video in the document.
    const discoveryStarted = new WeakMap<HTMLVideoElement, number>()
    const discoveryWaitMs = 12_000
    const knownVideos = new Set<HTMLVideoElement>()
    const observedVideoSizes = new WeakSet<HTMLVideoElement>()
    let videoResizeObserver: ResizeObserver | null = null
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
        resourceFingerprint(resource), resource.url, resource.title || '', resource.filename || '',
        resource.kind, resource.mimeType || '', resource.size || 0, resource.estimatedSize || 0,
        resource.width || 0, resource.height || 0, resource.bandwidth || 0, resource.duration || 0,
        resource.isLive === true ? 1 : resource.isLive === false ? 0 : '', resource.inspected ? 1 : 0,
        (resource.variants || []).map(variant => `${variant.url}:${variant.width || 0}:${variant.height || 0}:${variant.bandwidth || 0}`).join(','),
        includePlaybackEvidence ? (resource.playbackUrls || []).join(',') : '',
        includePlaybackEvidence ? (resource.playbackPatterns || []).join(',') : '',
        includePlaybackEvidence ? (resource.playbackUrlHashes || []).join(',') : '',
      ].join('|'))
      .sort()
      .join('\n')
    const addResource = (resource: MediaResource) => {
      const before = resourcePresentationSignature()
      replaceResources(mergeResources([...resources.values()], resource, 40))
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
      name: 'hls-downloader-media-panel', position: 'inline', anchor: 'body',
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
          :host{all:initial}*{box-sizing:border-box}button{font:13px system-ui,sans-serif;letter-spacing:0}
          ${THEME_TOKENS_CSS}
          ${THEME_BASE_CSS}
          .wrap{display:none;position:fixed;z-index:var(--z-extension-overlay);color:var(--text);filter:drop-shadow(0 6px 12px var(--shadow))}.wrap.open{display:block}
          .panel{display:none;width:min(344px,calc(100vw - 20px));max-height:min(520px,calc(100vh - 20px));background:var(--surface);border:1px solid var(--overlay-border);border-radius:9px;overflow:hidden}.open .panel{display:block}
          header{display:flex;align-items:center;justify-content:space-between;padding:7px 8px 7px 9px;border-bottom:1px solid var(--border);background:var(--surface-2);color:var(--text);font:600 12px system-ui;cursor:grab;touch-action:none}.title{display:flex;align-items:center;gap:6px}.title img{width:16px;height:16px;border-radius:4px}.head-actions{display:flex;align-items:center;gap:4px}
          .pin,.close{height:27px;border:0;border-radius:5px;background:var(--surface-3);color:var(--text);cursor:pointer}.pin{padding:0 8px;font:11px system-ui}.pin.active{background:color-mix(in srgb,var(--green) 18%,var(--surface-3));color:var(--green-ink)}.close{display:grid;place-items:center;width:27px;font:700 18px/1 system-ui}.pin:hover,.close:hover{background:color-mix(in srgb,var(--primary) 14%,var(--surface-3))}.list{max-height:calc(min(520px,calc(100vh - 20px)) - 78px);overflow-y:auto;overscroll-behavior:contain}
          .item{padding:9px 10px;border-bottom:1px solid var(--border)}.item:last-child{border-bottom:0}.item:hover{background:var(--surface-2)}.meta{min-width:0}.name{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;font:600 12px/1.35 system-ui;overflow-wrap:anywhere;color:var(--text)}.kind{overflow:hidden;color:var(--muted);font:10.5px/1.35 system-ui;margin-top:3px;text-overflow:ellipsis;white-space:nowrap}.resource-url{display:block;margin-top:4px;color:var(--faint);font:10px/1.4 ui-monospace,SFMono-Regular,Consolas,monospace;overflow-wrap:anywhere;user-select:text}.quality-select{width:min(184px,100%);height:29px;margin-top:6px;font-size:11.5px}.item-actions{display:flex;gap:5px;margin-top:8px}.download{min-width:0;flex:1;height:29px;border:0;border-radius:6px;background:var(--primary);color:var(--on-primary);padding:4px 6px;cursor:pointer;font-weight:600;font-size:11px}.download:hover{background:var(--primary-hover)}.download[disabled]{cursor:default;opacity:.6}.download.push-tv{background:color-mix(in srgb,var(--purple) 82%,var(--text))}.download.push-tv:hover{background:var(--purple)}.download.cast{background:color-mix(in srgb,var(--green) 82%,var(--text))}.download.cast:hover{background:var(--green)}.result{padding:7px 10px;background:color-mix(in srgb,var(--green) 14%,var(--surface));color:var(--green-ink);font:11px/1.4 system-ui}.result.error{background:color-mix(in srgb,var(--red) 12%,var(--surface));color:var(--red-ink)}
          .video-buttons{position:fixed;inset:0;z-index:var(--z-extension-video);pointer-events:none}.video-action-group{position:fixed;display:flex;align-items:center;gap:4px;pointer-events:auto}.video-download{position:relative;display:flex;align-items:center;gap:7px;height:34px;padding:0 12px;border:1px solid color-mix(in srgb,var(--primary) 60%,transparent);border-radius:7px;background:var(--primary);color:var(--on-primary);box-shadow:0 3px 8px var(--shadow);cursor:grab;touch-action:none;user-select:none;-webkit-user-select:none;font:600 12px system-ui;transition:background-color .16s ease,transform .16s ease}.video-download:active{cursor:grabbing}.video-download:hover{background:var(--primary-hover)}.video-download.identifying{border-color:var(--overlay-border);background:color-mix(in srgb,var(--surface) 88%,var(--primary));color:var(--muted)}.video-download.identifying:hover{background:color-mix(in srgb,var(--surface) 88%,var(--primary))}.video-download img{width:18px;height:18px;border-radius:4px}.video-download b{display:inline-grid;place-items:center;min-width:18px;height:18px;padding:0 4px;border-radius:9px;background:var(--on-primary);color:var(--primary);font:700 10px system-ui}.video-more{display:grid;width:34px;height:34px;place-items:center;border:1px solid var(--overlay-border);border-radius:7px;background:var(--surface);color:var(--primary);box-shadow:0 3px 8px var(--shadow);cursor:pointer;font:700 18px/1 system-ui}.video-more:hover{background:var(--surface-2)}
          .video-hover{position:absolute;top:38px;left:0;width:292px;padding:10px;background:var(--surface);color:var(--text);border:1px solid var(--overlay-border);border-radius:8px;box-shadow:0 4px 8px var(--shadow);opacity:0;visibility:hidden;transform:translateY(-3px);pointer-events:auto;transition:opacity .16s ease,transform .16s ease,visibility 0s linear .16s}.video-hover::before{content:'';position:absolute;left:0;right:0;top:-10px;height:10px}.video-hover.above{top:auto;bottom:38px;transform:translateY(3px)}.video-hover.above::before{top:auto;bottom:-10px}.video-action-group:hover .video-hover,.video-action-group:focus-within .video-hover,.video-action-group.hover-open .video-hover{opacity:1;visibility:visible;transform:translateY(0);transition-delay:.05s,.05s,0s}.video-action-group.dragging .video-hover{opacity:0;visibility:hidden;transition:none}.hover-head{display:flex;align-items:flex-start;justify-content:space-between;gap:8px}.hover-title{min-width:0;display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;color:var(--text);font:650 12px/1.35 system-ui;overflow-wrap:anywhere}.hover-state{flex:none;color:var(--green);font:600 10px/1.35 system-ui;white-space:nowrap}.hover-copy{margin-top:4px;color:var(--muted);font:11px/1.4 system-ui}.hover-facts{display:flex;flex-wrap:wrap;gap:4px;margin-top:7px}.hover-fact{height:20px;padding:2px 6px;border-radius:5px;background:var(--surface-2);color:var(--muted);font:600 10px/16px system-ui;white-space:nowrap}.hover-source{margin-top:7px;overflow:hidden;color:var(--faint);font:10px/1.35 ui-monospace,SFMono-Regular,Consolas,monospace;text-overflow:ellipsis;white-space:nowrap}.hover-actions{display:flex;gap:5px;margin-top:9px}.hover-action{min-width:0;flex:1;height:28px;padding:0 6px;border:0;border-radius:6px;background:var(--surface-3);color:var(--text);cursor:pointer;font:600 10.5px/1 system-ui}.hover-action:hover{background:color-mix(in srgb,var(--primary) 12%,var(--surface-3))}.hover-action.primary{background:var(--primary);color:var(--on-primary)}.hover-action.primary:hover{background:var(--primary-hover)}.hover-action[disabled]{cursor:default;opacity:.55}
          button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}
          /* 统一按钮微交互。视频悬浮按钮与面板按钮此前要么完全没有 transition，
             要么声明了 transform 过渡却没有任何状态去改 transform（等于死代码），
             点击时只剩颜色硬切。这里给它们一套与 .hlsd-button（transform .12s）和
             Compose 工作台（hover 130ms / press 90ms）对齐的反馈。 */
          .video-download,.video-more,.hover-action,.pin,.close,.download{transition:background-color .15s ease,color .15s ease,transform .12s ease,box-shadow .15s ease}
          .video-download:hover,.video-more:hover{transform:translateY(-1px)}
          .video-download:hover{box-shadow:0 5px 14px var(--shadow)}
          .video-more:active{transform:scale(.92)}
          .video-download:active{transform:scale(.96);box-shadow:0 2px 6px var(--shadow)}
          .hover-action:active:not([disabled]),.pin:active,.close:active{transform:scale(.94)}
          .download:active:not([disabled]){transform:scale(.97)}
          /* 识别中是"还不可点"的状态，不给抬升和按压反馈，避免暗示可交互。 */
          .video-download.identifying:hover,.video-download.identifying:active{transform:none;box-shadow:0 3px 8px var(--shadow)}
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
        const panelWrap = element('div', 'wrap')
        const panel = element('div', 'panel')
        const header = element('header')
        const title = element('span', 'title', '当前视频')
        title.prepend(image())
        const headActions = element('div', 'head-actions')
        const pin = element('button', 'pin', '固定') as HTMLButtonElement
        pin.type = 'button'
        pin.title = '固定展开'
        const close = element('button', 'close', '×') as HTMLButtonElement
        close.type = 'button'
        close.title = '折叠'
        close.setAttribute('aria-label', '折叠')
        headActions.append(pin, close)
        header.append(title, headActions)
        const result = element('div', 'result')
        result.hidden = true
        const list = element('div', 'list')
        panel.append(header, result, list)
        panelWrap.append(panel)
        const videoButtons = element('div', 'video-buttons')
        root.append(style, panelWrap, videoButtons)
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
    const setOpen = (open: boolean) => {
      if (open && collapseTimer) {
        clearTimeout(collapseTimer)
        collapseTimer = null
      }
      wrap?.classList.toggle('open', open)
      if (open) fitPanel()
    }
    const pinButton = ui.shadow.querySelector<HTMLButtonElement>('.pin')
    const setPinned = (value: boolean) => {
      pinned = value
      pinButton?.classList.toggle('active', value)
      if (pinButton) pinButton.textContent = value ? '已固定' : '固定'
      if (value) setOpen(true)
    }
    wrap?.addEventListener('mouseenter', () => {
      setOpen(true)
    })
    wrap?.addEventListener('mouseleave', () => {
      // 选中链接需要逐个桌面确认，离开面板不能使剩余链接消失。
      if (pinned || dragged || selectionMode) return
      collapseTimer = setTimeout(() => setOpen(false), 450)
    })
    pinButton?.addEventListener('click', () => setPinned(!pinned))
    ui.shadow.querySelector('.close')?.addEventListener('click', () => {
      if (pinned) setPinned(false)
      setOpen(false)
    })
    // Older builds persisted panelPinned/panelPosition across every site.
    // Clear that migration residue once: visibility and positions now belong
    // only to the active playback session in this page.
    void browser.storage.local.remove(['panelPinned', 'panelPosition', 'videoButtonPosition']).catch(() => undefined)
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
      button.dataset.sendKey = key
      button.dataset.sendFallback = fallbackLabel
      const state = resourceSendStates.get(key)
      const label = button.querySelector<HTMLElement>('.download-label')
      if (label) label.textContent = state?.label || fallbackLabel
      else button.textContent = state?.label || fallbackLabel
      if (state?.disabled) button.setAttribute('disabled', '')
      else button.removeAttribute('disabled')
      if (state?.error) button.title = state.error
      else if (button.dataset.sendError) button.removeAttribute('title')
      button.dataset.sendError = state?.error || ''
    }

    const setSendState = (resource: MediaResource, button: HTMLButtonElement, label: string, disabled: boolean, fallbackLabel = '下载', action: OverlayAction = 'download', error = '') => {
      const key = overlaySendKey(resourceFingerprint(resource), action)
      if (label) resourceSendStates.set(key, { label, disabled, error })
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
    }

    const sendResource = (resource: MediaResource, button: HTMLButtonElement) => {
      const key = resourceFingerprint(resource)
      if (resourceSendStates.get(key)?.disabled) return
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      setSendState(resource, button, '发送中', true)
      void withDeadline(runtimeMessage({ type: 'download-now', resource }), 10_000, '下载器响应超时，请重试').then(async response => {
        if (!response?.ok) throw new Error(response?.error || '桌面端未接受请求')
        setSendState(resource, button, '等待确认', true)
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = '请在桌面下载窗口确认文件名和保存位置' }
        if (!await waitForDownloadConfirmation(runtimeMessage, String(response.handoff?.id || ''))) {
          setSendState(resource, button, '', false)
          if (result) result.textContent = '已取消下载'
          return
        }
        setSendState(resource, button, '已加入', true)
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `已加入下载队列：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
        ctx.setTimeout(() => setSendState(resource, button, '', false), 2_500)
      }).catch(reason => {
        const error = reason?.message || String(reason) || '发送失败'
        setSendState(resource, button, '重试', false, '下载', 'download', error)
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = error }
      })
    }

    const pushToTv = (resource: MediaResource, button: HTMLButtonElement) => {
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      const key = overlaySendKey(resourceFingerprint(resource), 'tvbox')
      if (resourceSendStates.get(key)?.disabled) return
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
        if (!response?.ok) throw new Error(response?.error || '电视推送失败')
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = '请在桌面下载器选择 TVBox 设备' }
        const status = await waitForResult(String(response.id || ''))
        if (status.status !== 'done') throw new Error(status.message || '电视推送未完成')
        setSendState(resource, button, '已发送', true, overlayActionFallback('tvbox'), 'tvbox')
        if (result) result.textContent = status.message || 'TVBox 推送成功'
        ctx.setTimeout(() => setSendState(resource, button, '', false, overlayActionFallback('tvbox'), 'tvbox'), 2_000)
      }).catch(reason => {
        setSendState(resource, button, '重试', false, overlayActionFallback('tvbox'), 'tvbox', reason?.message || String(reason) || '推送失败')
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '推送失败' }
      })
    }

    const castResource = (resource: MediaResource, button: HTMLButtonElement) => {
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      const key = overlaySendKey(resourceFingerprint(resource), 'cast')
      if (resourceSendStates.get(key)?.disabled) return
      setSendState(resource, button, '等待选择', true, overlayActionFallback('cast'), 'cast')
      void runtimeMessage({ type: 'cast-to-device', resource }).then(async response => {
        if (!response?.ok) throw new Error(response?.error || '投屏请求失败')
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = '请在桌面下载器选择投屏设备' }
        const deadline = Date.now() + 130_000
        let status: any = null
        while (Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 1_000))
          status = await runtimeMessage({ type: 'media-push-status', requestId: String(response.id || '') }).catch(() => null)
          status = mediaPushTerminalResult(status); if (status) break
        }
        if (status?.status !== 'done') throw new Error(status?.message || '投屏未完成')
        setSendState(resource, button, '已发送', true, overlayActionFallback('cast'), 'cast')
        if (result) result.textContent = status.message || '投屏成功'
        ctx.setTimeout(() => setSendState(resource, button, '', false, overlayActionFallback('cast'), 'cast'), 2_000)
      }).catch(reason => {
        setSendState(resource, button, '重试', false, overlayActionFallback('cast'), 'cast', reason?.message || String(reason) || '投屏请求失败')
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '投屏请求失败' }
      })
    }

    const openVideoPanel = (video: HTMLVideoElement, rect: DOMRect) => {
      if (!wrap) return
      activeVideo = video
      activePlayback = playbackByVideo.get(video) || null
      selectionMode = false
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

    const updateVideoButtons = () => {
      if (ctx.isInvalid) return
      const layer = ui.shadow.querySelector<HTMLElement>('.video-buttons')
      if (!layer) return
      // Players emit timeupdate while the pointer is down. Replacing the
      // control in that interval cancels pointer capture before it can move.
      if (videoControlDragging) return
      let visible = 0
      const buttonBounds = videoButtonBounds()
      const videos = currentVideos()
        .map(video => ({ video, rect: video.getBoundingClientRect(), playback: playbackByVideo.get(video) || null }))
        .filter(({ video, rect, playback }) => Boolean(playback)
          && (video === activeVideo || (!video.paused && !video.ended))
          && rect.width >= 180 && rect.height >= 100
          && rect.bottom >= 0 && rect.top <= innerHeight && rect.right >= 0 && rect.left <= innerWidth)
        .map(item => {
          const candidates = playerPlaybackResources([...resources.values()], item.playback, 8)
          const exact = candidates.filter(resource => item.playback?.sourceUrls.some(source => resourceMatchesPlaybackSource(resource, source)))
          const matched = candidates.length > 0
          const expired = Date.now() - (discoveryStarted.get(item.video) || item.playback?.startedAt || Date.now()) >= discoveryWaitMs
          // 无法截获分片字节的 MSE 播放器仍可手选页面已发现的清单，但不能假称已匹配并一键下载。
          const fallback = !matched && expired && item.playback?.sourceUrls.some(source => source.startsWith('blob:'))
            ? visiblePlaybackResources([...resources.values()], item.playback, 8) : []
          const choices = (exact.length ? exact : matched ? candidates : fallback)
            .sort((left, right) => resourceRank(right) - resourceRank(left) || (right.height || 0) - (left.height || 0) || (right.bandwidth || 0) - (left.bandwidth || 0) || (right.size || right.estimatedSize || 0) - (left.size || left.estimatedSize || 0))
          return { ...item, choices, matched }
        })
        .sort((left, right) => Number(right.video === activeVideo) - Number(left.video === activeVideo)
          || right.rect.width * right.rect.height - left.rect.width * left.rect.height)
      const signature = JSON.stringify([
        innerWidth, innerHeight, buttonBounds, selectionMode, [...resourceSendStates],
        videos.map(({ video, rect, playback, choices, matched }) => [
          mediaOwner(video), rect.x, rect.y, rect.width, rect.height, playback?.sourceUrls, playback?.startedAt,
          resourcePresentationSignature(choices, false), matched, videoButtonPositions.get(video),
          Date.now() - (discoveryStarted.get(video) || playback?.startedAt || Date.now()) >= discoveryWaitMs,
        ]),
      ])
      // 时间进度事件不能不断销毁按钮；只在资源、布局或发送状态变化时重建。
      if (signature === renderedVideoSignature) return
      renderedVideoSignature = signature
      layer.replaceChildren()
      // 每个播放器独立匹配自己的 currentSrc 或 SourceBuffer 响应，避免串到广告或其他视频。
      videos.forEach(({ video, rect, playback, choices, matched }) => {
        if (!playback) return
        if (!shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: choices.length })) return
        const identifying = choices.length === 0
        const discoveryExpired = identifying && Date.now() - (discoveryStarted.get(video) || playback.startedAt) >= discoveryWaitMs
        // playerPlaybackResources already enforces the ambiguity boundary for
        // blob/MSE (per-SourceBuffer evidence).
        // Requiring a literal currentSrc match again made a uniquely resolved
        // MSE video open a pointless one-item picker.
        const oneClickChoice = matched && choices.length === 1
        visible += 1
        const buttonWidth = 156
        const buttonHeight = 34
        const controlsWidth = buttonWidth + (oneClickChoice ? 38 : 0)
        const group = document.createElement('div')
        group.className = 'video-action-group'
        const button = document.createElement('button')
        button.type = 'button'; button.className = `video-download${identifying ? ' identifying' : ''}`
        if (oneClickChoice) button.dataset.resourceId = choices[0].id
        button.setAttribute('aria-label', discoveryExpired ? '重新识别当前视频资源' : identifying ? '正在识别当前播放的视频资源' : oneClickChoice ? '下载当前视频' : `选择 ${choices.length} 个视频资源`)
        if (identifying && !discoveryExpired) button.setAttribute('aria-disabled', 'true')
        const besidePlayer = rect.right + 8
        const defaultLeft = besidePlayer + controlsWidth <= buttonBounds.right - 8
          ? besidePlayer
          : Math.max(buttonBounds.left + 8, Math.min(rect.right - controlsWidth - 8, buttonBounds.right - controlsWidth - 8))
        const defaultTop = Math.max(buttonBounds.top + 8, Math.min(rect.top + 8, buttonBounds.bottom - buttonHeight - 8))
        const saved = videoButtonPositions.get(video)
        const position = clampVideoButtonPosition(saved || { x: defaultLeft, y: defaultTop },
          { width: controlsWidth, height: buttonHeight }, buttonBounds)
        group.style.left = `${position.x}px`
        group.style.top = `${position.y}px`
        const icon = document.createElement('img'); icon.src = browser.runtime.getURL('/icon-32.png'); icon.alt = ''
         const fallbackLabel = discoveryExpired ? '重新识别' : identifying ? '正在识别' : oneClickChoice ? '下载视频' : '选择资源'
         const label = document.createElement('span'); label.className = 'download-label'; label.textContent = fallbackLabel
        button.append(icon, label)
        if (choices.length > 1) { const count = document.createElement('b'); count.textContent = String(choices.length); button.append(count) }
        let videoDragged = false
        button.addEventListener('pointerdown', event => {
          if (event.button !== 0) return
          event.preventDefault()
          event.stopPropagation()
          button.setPointerCapture(event.pointerId)
          videoDragged = false
          videoControlDragging = true
          group.classList.add('dragging')
          const startX = event.clientX; const startY = event.clientY
          const startLeft = group.offsetLeft; const startTop = group.offsetTop
          const move = (next: PointerEvent) => {
            if (next.pointerId !== event.pointerId) return
            videoDragged ||= Math.abs(next.clientX - startX) + Math.abs(next.clientY - startY) > 4
            const height = button.offsetHeight || buttonHeight
            const position = clampVideoButtonPosition(
              { x: startLeft + next.clientX - startX, y: startTop + next.clientY - startY },
              { width: controlsWidth, height },
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
            if (videoDragged && next.type !== 'pointercancel') {
              videoButtonPositions.set(video, { x: group.offsetLeft, y: group.offsetTop })
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
          if (videoDragged) {
            event.preventDefault()
            event.stopImmediatePropagation()
            videoDragged = false
            return
          }
           if (identifying) {
             event.preventDefault()
             event.stopImmediatePropagation()
             if (discoveryExpired) {
               discoveryStarted.set(video, Date.now())
               void handleRuntimeMessage({ type: 'rescan-media' })
               ctx.setTimeout(scheduleVideoButtons, discoveryWaitMs + 20)
             }
             return
           }
           if (oneClickChoice) {
             event.preventDefault()
             event.stopImmediatePropagation()
             sendResource(choices[0], button)
             return
           }
          // Ambiguous MSE resources still require an evidence selection panel.
          openVideoPanel(video, rect)
        })
        if (oneClickChoice) applySendState(choices[0], button, fallbackLabel)
        group.append(button)
        if (oneClickChoice) {
          const more = document.createElement('button')
          more.type = 'button'; more.className = 'video-more'; more.textContent = '⋯'
          more.title = '更多操作：投屏或推送当前媒体链接'
          more.setAttribute('aria-label', '更多操作：投屏或推送当前媒体链接')
          more.addEventListener('pointerdown', event => event.stopPropagation())
          more.addEventListener('click', event => {
            event.preventDefault(); event.stopPropagation()
            openVideoPanel(video, rect)
          })
          group.append(more)
        }
        const hover = document.createElement('div')
        hover.className = 'video-hover'
        hover.setAttribute('role', 'group')
        const hoverId = `hlsd-video-hover-${visible}-${choices[0]?.id?.slice(0, 12) || 'identifying'}`
        hover.id = hoverId
        button.setAttribute('aria-describedby', hoverId)
        if (identifying) {
          const head = document.createElement('div'); head.className = 'hover-head'
          const hoverTitle = document.createElement('div'); hoverTitle.className = 'hover-title'; hoverTitle.textContent = '当前播放的视频'
          const state = document.createElement('span'); state.className = 'hover-state'; state.textContent = discoveryExpired ? '未匹配' : '识别中'
          const copy = document.createElement('div'); copy.className = 'hover-copy'; copy.textContent = discoveryExpired
            ? '未找到能确认属于此播放器的资源。可重新识别，或刷新网页后重新播放；插件弹窗可查看本页已发现的资源。'
            : '正在匹配实际播放资源，确认后即可下载或投屏。'
          head.append(hoverTitle, state); hover.append(head, copy)
        } else if (oneClickChoice) {
          const choice = choices[0]
          const details = overlayResourceDetails(choice)
          const head = document.createElement('div'); head.className = 'hover-head'
          const hoverTitle = document.createElement('div'); hoverTitle.className = 'hover-title'; hoverTitle.textContent = details.title
          const state = document.createElement('span'); state.className = 'hover-state'; state.textContent = details.state
          const facts = document.createElement('div'); facts.className = 'hover-facts'
          details.facts.forEach(value => { const fact = document.createElement('span'); fact.className = 'hover-fact'; fact.textContent = value; facts.append(fact) })
          const source = document.createElement('div'); source.className = 'hover-source'; source.textContent = details.source; source.title = details.source
          const actions = document.createElement('div'); actions.className = 'hover-actions'
          const download = document.createElement('button'); download.type = 'button'; download.className = 'hover-action primary'; download.textContent = '下载'
          const cast = document.createElement('button'); cast.type = 'button'; cast.className = 'hover-action'; cast.textContent = '投屏'
          const tvbox = document.createElement('button'); tvbox.type = 'button'; tvbox.className = 'hover-action'; tvbox.textContent = 'TVBox'
          download.addEventListener('click', event => { event.preventDefault(); event.stopPropagation(); sendResource(choice, download) })
          cast.addEventListener('click', event => { event.preventDefault(); event.stopPropagation(); castResource(choice, cast) })
          tvbox.addEventListener('click', event => { event.preventDefault(); event.stopPropagation(); pushToTv(choice, tvbox) })
          applySendState(choice, download, '下载')
          applySendState(choice, cast, '投屏', 'cast')
          applySendState(choice, tvbox, 'TVBox', 'tvbox')
          actions.append(download, cast, tvbox)
          head.append(hoverTitle, state); hover.append(head, facts, source, actions)
          const fp = resourceFingerprint(choice)
          const error = resourceSendStates.get(fp)?.error
            || resourceSendStates.get(overlaySendKey(fp, 'cast'))?.error
            || resourceSendStates.get(overlaySendKey(fp, 'tvbox'))?.error
          if (error) {
            const detail = document.createElement('div')
            detail.className = 'result error send-error'
            detail.textContent = error
            detail.setAttribute('role', 'alert')
            hover.append(detail)
          }
        } else {
          const details = overlayResourceDetails(choices[0])
          const head = document.createElement('div'); head.className = 'hover-head'
          const hoverTitle = document.createElement('div'); hoverTitle.className = 'hover-title'; hoverTitle.textContent = `${choices.length} 个可用资源`
          const state = document.createElement('span'); state.className = 'hover-state'; state.textContent = '需要选择'
          const copy = document.createElement('div'); copy.className = 'hover-copy'; copy.textContent = matched
            ? `优先候选：${details.title}` : '已发现页面媒体，但尚未确认播放器归属。请核对标题、清晰度和来源后选择。'
          const facts = document.createElement('div'); facts.className = 'hover-facts'
          details.facts.slice(0, 4).forEach(value => { const fact = document.createElement('span'); fact.className = 'hover-fact'; fact.textContent = value; facts.append(fact) })
          const actions = document.createElement('div'); actions.className = 'hover-actions'
          const choose = document.createElement('button'); choose.type = 'button'; choose.className = 'hover-action primary'; choose.textContent = '查看并选择'
          choose.addEventListener('click', event => { event.preventDefault(); event.stopPropagation(); openVideoPanel(video, rect) })
          actions.append(choose); head.append(hoverTitle, state); hover.append(head, copy, facts, actions)
        }
        const positionHover = () => {
          const groupRect = group.getBoundingClientRect()
          const hoverWidth = Math.max(220, Math.min(292, innerWidth - 16))
          hover.style.width = `${hoverWidth}px`
          const relativeLeft = Math.max(8 - groupRect.left, Math.min(0, innerWidth - 8 - hoverWidth - groupRect.left))
          hover.style.left = `${relativeLeft}px`
          ctx.requestAnimationFrame(() => {
            const hoverHeight = hover.offsetHeight || 150
            hover.classList.toggle('above', groupRect.bottom + 6 + hoverHeight > innerHeight - 8 && groupRect.top >= hoverHeight + 8)
          })
        }
        let hoverDismissTimer: ReturnType<typeof setTimeout> | null = null
        const keepHoverOpen = () => {
          if (hoverDismissTimer) clearTimeout(hoverDismissTimer)
          hoverDismissTimer = null
          group.classList.add('hover-open')
          positionHover()
        }
        const dismissHover = () => {
          if (hoverDismissTimer) clearTimeout(hoverDismissTimer)
          hoverDismissTimer = setTimeout(() => {
            hoverDismissTimer = null
            if (shouldKeepHoverOpen(group.matches(':hover'), group.contains(ui.shadow.activeElement))) return
            group.classList.remove('hover-open')
            scheduleVideoButtons()
          }, HOVER_DISMISS_DELAY_MS)
        }
        group.addEventListener('pointerenter', keepHoverOpen)
        group.addEventListener('pointerleave', dismissHover)
        group.addEventListener('focusin', keepHoverOpen)
        group.addEventListener('focusout', dismissHover)
        group.append(hover)
        layer.append(group)
        positionHover()
      })
      if (!visible && !selectionMode) {
        if (pinned) setPinned(false)
        setOpen(false)
        panelPosition = null
      }
    }

    const render = () => {
      if (ctx.isInvalid) return
      const list = ui.shadow.querySelector('.list')
      if (!list) return
      const selectionResources = selectionResourceIds
        ? [...resources.values()].filter(resource => selectionResourceIds?.has(resourceFingerprint(resource)))
        : [...resources.values()]
      // 已有播放器归属证据时，展开面板必须沿用同一资源，不能重新按全页码率挑选。
      const matched = playerPlaybackResources([...resources.values()], activePlayback, 8)
      const entries = selectionMode && !activePlayback
        ? compactResources(selectionResources, 40).slice(0, 20)
        : matched.length ? matched : visiblePlaybackResources([...resources.values()], activePlayback, 8)
      const signature = JSON.stringify([selectionMode, selectionResourceIds?.size, entries.map(resource => [
        resource.id, resource.url, resource.title, resource.filename, resource.kind,
        resource.size, resource.estimatedSize, resource.width, resource.height, resource.bandwidth,
        resource.quality, resource.duration, resource.isLive, resource.variants,
        preferredVariants.get(resourceFingerprint(resource)),
      ]), [...resourceSendStates]])
      // 播放进度和重复网络观察不能不断替换用户正在点击的按钮及清晰度控件。
      if (signature === renderedListSignature) { updateVideoButtons(); return }
      renderedListSignature = signature
      list.replaceChildren()
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      if (selectionMode && selectionResourceIds?.size === 0 && result) {
        result.hidden = false
        result.classList.add('error')
        result.dataset.source = 'selection'
        result.textContent = '选中的内容里没有可下载链接'
      } else if (result?.dataset.source === 'selection') {
        result.hidden = true
        result.classList.remove('error')
        delete result.dataset.source
      }
      entries.forEach(resource => {
        const row = document.createElement('div'); row.className = 'item'
        const meta = document.createElement('div'); meta.className = 'meta'
        const name = document.createElement('div'); name.className = 'name'; name.title = resource.title || resource.filename || resource.url; name.textContent = resource.title || resource.filename || resource.url.split('/').pop() || resource.url
        let host = ''; try { host = new URL(resource.url).host } catch {}
        const quality = resource.quality || resourceQuality(resource.url, resource.height)
        const streamMode = resource.isLive === true ? '直播' : ''
        const duration = resource.duration ? formatDuration(resource.duration) : ''
        const bandwidth = resource.bandwidth ? `${(resource.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''
        const likelySize = resource.size || resource.estimatedSize || 0
        const sizeLabel = resource.size ? formatBytes(resource.size) : likelySize ? `约 ${formatBytes(likelySize)}` : '大小未知'
        const kind = document.createElement('div'); kind.className = 'kind'; kind.textContent = [resource.kind.toUpperCase(), streamMode, quality, resource.width && resource.height ? `${resource.width}×${resource.height}` : '', bandwidth, duration, sizeLabel, host].filter(Boolean).join(' · ')
        const resourceLocation = safeResourceLocation(resource.url)
        const resourceUrl = document.createElement('code'); resourceUrl.className = 'resource-url'; resourceUrl.title = resourceLocation; resourceUrl.textContent = resourceLocation
        const preference = preferredVariants.get(resourceFingerprint(resource))
        const selectedIndex = preference
          ? resource.variants?.findIndex(variant => (variant.height || 0) === preference.height
            && (variant.bandwidth || 0) === preference.bandwidth) ?? -1 : -1
        let selected = selectedIndex >= 0 ? selectMediaVariant(resource, resource.variants![selectedIndex]) : resource
        const actions = document.createElement('div'); actions.className = 'item-actions'
        const button = document.createElement('button'); button.type = 'button'; button.className = 'download'; button.textContent = '下载'
        button.addEventListener('click', () => sendResource(selected, button))
        applySendState(selected, button, '下载')
        if (resource.variants?.length) {
          const select = document.createElement('select')
          select.className = 'hlsd-select quality-select'
          select.setAttribute('aria-label', '选择视频清晰度')
          const automatic = document.createElement('option')
          automatic.value = 'auto'
          automatic.textContent = '自动（最高）'
          select.append(automatic)
          resource.variants.forEach((variant, index) => {
            const option = document.createElement('option')
            option.value = String(index)
            option.textContent = [variant.quality || (variant.height ? `${variant.height}p` : '线路'), variant.bandwidth ? `${(variant.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''].filter(Boolean).join(' · ')
            select.append(option)
          })
          select.value = selectedIndex >= 0 ? String(selectedIndex) : 'auto'
          select.addEventListener('change', () => {
            const variant = resource.variants?.[Number(select.value)]
            selected = variant ? selectMediaVariant(resource, variant) : resource
            if (variant) preferredVariants.set(resourceFingerprint(resource), { height: variant.height || 0, bandwidth: variant.bandwidth || 0 })
            else preferredVariants.delete(resourceFingerprint(resource))
            applySendState(selected, button, '下载'); applySendState(selected, pushButton, overlayActionFallback('tvbox'), 'tvbox'); applySendState(selected, castButton, overlayActionFallback('cast'), 'cast')
          })
          meta.append(name, kind, resourceUrl, select)
        } else {
          meta.append(name, kind, resourceUrl)
        }
        const pushButton = document.createElement('button'); pushButton.type = 'button'; pushButton.className = 'download push-tv'; pushButton.textContent = 'TVBox'
        pushButton.title = '直接推送当前媒体链接到 TVBox'
        pushButton.addEventListener('click', () => pushToTv(selected, pushButton))
        applySendState(selected, pushButton, overlayActionFallback('tvbox'), 'tvbox')
        const castButton = document.createElement('button'); castButton.type = 'button'; castButton.className = 'download cast'; castButton.textContent = '投屏'
        castButton.title = '直接投屏当前媒体链接到 DLNA 或 Chromecast'
        castButton.addEventListener('click', () => castResource(selected, castButton))
        applySendState(selected, castButton, overlayActionFallback('cast'), 'cast')
        actions.append(button, pushButton, castButton)
        row.append(meta, actions); list.append(row)
      })
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
      positionFrame = ctx.requestAnimationFrame(() => { positionFrame = 0; updateVideoButtons() })
    }
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
    ctx.addEventListener(document, 'fullscreenchange', scheduleVideoButtons)

    videoResizeObserver = typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver(() => scheduleVideoButtons())
    ctx.onInvalidated(() => videoResizeObserver?.disconnect())
    const trackVideo = (video: HTMLVideoElement) => {
      knownVideos.add(video)
      if (observedVideoSizes.has(video)) return
      observedVideoSizes.add(video)
      videoResizeObserver?.observe(video)
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
    ) => {
      const kind = playbackSource ? classifyPlaybackSource(url, mimeType) : classifyResource(url, mimeType)
      if (!kind) return
      let filename = ''
      try { filename = decodeURIComponent(new URL(url).pathname.split('/').pop() || '') } catch {}
      const resource = {
        // MAIN 桥仅接收解析结果，不能用网页事件覆盖请求身份或凭据。
        ...(inspection?.inspected === true ? {
          inspected: true, manifestType: inspection.manifestType, isLive: inspection.isLive,
          lowLatencyLive: inspection.lowLatencyLive, partOnlyLive: inspection.partOnlyLive,
          duration: inspection.duration, variants: inspection.variants, audioTracks: inspection.audioTracks,
          renditionUrls: inspection.renditionUrls, playbackUrls: inspection.playbackUrls,
          playbackUrlHashes: inspection.playbackUrlHashes, playbackPatterns: inspection.playbackPatterns,
          width: inspection.width, height: inspection.height, bandwidth: inspection.bandwidth,
          estimatedSize: inspection.estimatedSize, quality: inspection.quality,
        } : {}),
        id: resourceId(url), url, kind, mimeType, pageUrl: location.href,
        title: pageMediaTitle() || filename, filename, seenAt: Date.now(),
        evidence: evidence.length ? evidence.slice(0, 16) : [playbackSource ? 'current_src' : 'page_observation'],
        owner: owner || (playbackSource ? 'media-element' : 'page'),
        confidence: boundedConfidence(confidence || (playbackSource ? 0.98 : 0.58)),
        replayContext: { method: 'GET', page_url: location.href.split('#', 1)[0] },
      }
      const changed = addResource(resource)
      if (changed) scheduleRender()
      const write = runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
      pendingResourceWrites.add(write)
      void write.finally(() => pendingResourceWrites.delete(write))
      return write
    }

    const markVideoPlayback = (video: HTMLVideoElement, eventType: string) => {
      if (ctx.isInvalid) return
      const rect = video.getBoundingClientRect()
      if (rect.width < 180 || rect.height < 100 || rect.bottom < 0 || rect.top > innerHeight || rect.right < 0 || rect.left > innerWidth) return
      const previousActiveVideo = activeVideo
      const explicitActivation = ['play', 'playing', 'initializing'].includes(eventType)
      // timeupdate fires several times per second for every playing element.
      // It may refresh source evidence, but must not continuously steal the
      // active-player identity or rebuild the complete overlay DOM.
      if (explicitActivation || !activeVideo || !activeVideo.isConnected || activeVideo.paused || activeVideo.ended) {
        const changedPlayer = activeVideo !== video
        activeVideo = video
        if (changedPlayer) {
          selectionMode = false
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
        discoveryStarted.set(video, playback.startedAt)
        ctx.setTimeout(scheduleVideoButtons, discoveryWaitMs + 20)
        if (video === activeVideo) activePlayback = playback
        // currentSrc is immediate, player-specific evidence. Register it here
        // instead of waiting for PerformanceObserver/webRequest: Firefox can
        // omit those observations for dynamically created iframe media.
        const seenDirectSources = new Set<string>()
        directSources.forEach(source => {
          if (!source.url || seenDirectSources.has(source.url)) return
          seenDirectSources.add(source.url)
          add(source.url, source.mimeType, true, ['current_src'], mediaOwner(video), 0.98)
        })
      } else {
        if (video === activeVideo) activePlayback = previousPlayback
        if (previousActiveVideo !== activeVideo) scheduleVideoButtons()
        return
      }
      render()
    }
    const markPlayback = (event: Event) => {
      const video = eventVideo(event)
      if (!video) return
      if (
        video.paused
        && !playbackByVideo.has(video)
        && ['loadedmetadata', 'loadeddata', 'timeupdate'].includes(event.type)
      ) return
      markVideoPlayback(video, event.type)
    }
    const syncPlayingVideos = () => {
      currentVideos().forEach(video => {
        if (!video.paused && !video.ended) markVideoPlayback(video, 'sync')
      })
    }
    const observedMediaRoots = new WeakSet<Document | ShadowRoot>()
    const mediaEvents = ['play', 'playing', 'loadedmetadata', 'loadeddata', 'timeupdate'] as const
    const registerMediaRoot = (root: Document | ShadowRoot) => {
      if (observedMediaRoots.has(root)) return
      observedMediaRoots.add(root)
      const discover = (node: ParentNode | Element) => {
        discoverSubtitlePlayers(node)
        if (node instanceof HTMLVideoElement) trackVideo(node)
        node.querySelectorAll<HTMLVideoElement>('video').forEach(trackVideo)
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
        while (current && visited < 1024) {
          inspect(current)
          visited += 1
          current = walker.nextNode() as Element | null
        }
      }
      discover(root)
      mediaEvents.forEach(type => ctx.addEventListener(root, type, markPlayback, { capture: true }))
      const mediaObserver = new MutationObserver(mutations => {
        // 播放器被移除时也要撤掉旧入口，并释放 ResizeObserver 对元素的引用。
        let playbackChanged = mutations.some(mutation => mutation.removedNodes.length > 0)
          && [...knownVideos].some(video => !video.isConnected)
        mutations.forEach(mutation => {
          if (mutation.type === 'childList') {
            mutation.addedNodes.forEach(node => {
              if (!(node instanceof Element)) return
              const containsMedia = node.matches('video,audio,source')
                || Boolean(node.querySelector('video,audio,source'))
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
    syncPlayingVideos()
    document.documentElement.setAttribute('data-hls-downloader-extension', '1')

    const handleResourceEvent = (event: Event) => {
      const detail = bridgeDetail(event)
      add(detail.url, detail.mimeType, false, detail.inspection?.inspected ? ['manifest_inspection', 'page_response'] : [], '', 0, detail.inspection)
    }
    const handleMseEvent = (event: Event) => {
      const detail = bridgeDetail(event)
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
        // Register that stream as the current player resource; `.m4s`/`.ts`
        // fragments still classify as null and stay out of the overlay.
        const ownerVideo = currentVideos().find(video =>
          mediaElementSources(video).some(source => source.url === blobUrl),
        )
        const owner = ownerVideo ? mediaOwner(ownerVideo) : `mse-source-buffer:${resourceId(blobUrl)}`
        add(mediaUrl, '', false, ['mse_source_buffer'], owner, 0.97)
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
      mediaElementSources(media).forEach(source => add(source.url, source.mimeType, true, ['current_src'], mediaOwner(media), 0.98))
    })
    document.querySelectorAll<HTMLSourceElement>('source').forEach(source => {
      const url = explicitElementSource(source)
      if (url) add(url, source.type || '', true)
    })
    const resourceObserver = new PerformanceObserver(list => list.getEntries().forEach(entry => add(entry.name)))
    resourceObserver.observe({ type: 'resource', buffered: true })
    ctx.onInvalidated(() => resourceObserver.disconnect())
    const handleRuntimeMessage = (message: any) => {
      if (message?.type === 'download-error') {
        selectionMode = true
        selectionResourceIds = null
        setOpen(true)
        render()
        const result = ui.shadow.querySelector<HTMLElement>('.result')
        if (result) {
          result.hidden = false
          result.classList.add('error')
          result.dataset.source = 'download'
          result.textContent = String(message.error || '下载请求失败，请重试')
        }
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
        const selection = window.getSelection(); if (!selection?.rangeCount) return
        const root = selection.getRangeAt(0).cloneContents()
        const urls = selectedDownloadUrls(
          [...root.querySelectorAll<HTMLAnchorElement>('a[href]')].map(anchor => anchor.getAttribute('href') || ''),
          selection.toString(),
          location.href,
        )
        selectionMode = true
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
        setOpen(true)
        render()
        return
      }
      if (message?.type === 'open-media-panel') {
        if (message.scope === 'tab') {
          activePlayback = null
          activeVideo = null
          selectionMode = true
          loadPageResources(location.href, 'tab')
        } else if (!activePlayback) selectionMode = true
        selectionResourceIds = null
        setOpen(true)
        render()
        return
      }
      if (message?.type === 'rescan-media') {
        // A popup-initiated rescan must collect fresh page evidence without
        // forcing the larger in-page panel open. Replay early MAIN-world MSE
        // observations, re-read current media elements, then persist/render.
        window.dispatchEvent(new Event('__hls_downloader_replay__'))
        document.querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video,audio').forEach(media => {
          mediaElementSources(media).forEach(source => add(source.url, source.mimeType, true, ['manual_rescan'], mediaOwner(media), 0.98))
        })
        document.querySelectorAll<HTMLSourceElement>('source').forEach(source => {
          const url = explicitElementSource(source)
          if (url) add(url, source.type || '', true, ['manual_rescan'])
        })
        performance.getEntriesByType('resource').forEach(entry => add(entry.name))
        syncPlayingVideos()
        scheduleRender()
        return Promise.all([...pendingResourceWrites]).then(() => ({ ok: true }))
      }
    }
    browser.runtime.onMessage.addListener(handleRuntimeMessage)
    ctx.onInvalidated(() => browser.runtime.onMessage.removeListener(handleRuntimeMessage))
    contentReady = true
    browser.runtime.onMessage.removeListener(earlyRuntimeListener)
    earlyRuntimeMessages.splice(0).forEach(handleRuntimeMessage)
    let currentPageUrl = pageKey(location.href)
    function loadPageResources(pageUrl: string, scope?: 'tab') {
      void runtimeMessage({ type: 'list', pageUrl, ...(scope ? { scope } : {}) }).then((stored: MediaResource[]) => {
        if (ctx.isInvalid || !Array.isArray(stored) || pageKey(location.href) !== pageKey(pageUrl)) return
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
      currentPageUrl = next
      activePlayback = null
      activeVideo = null
      selectionMode = false
      selectionResourceIds = null
      playbackByVideo = new WeakMap<HTMLVideoElement, PlaybackContext>()
      videoButtonPositions = new WeakMap<HTMLVideoElement, { x: number, y: number }>()
      panelPosition = null
      if (pinned) setPinned(false)
      setOpen(false)
      resources.clear(); render(); loadPageResources(location.href)
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
