import { browser } from 'wxt/browser'
import { clampOverlayPosition, overlayActionFallback, overlaySendKey, shouldShowMediaOverlay, type OverlayAction } from '../lib/mediaOverlay'
import { classifyPlaybackSource, classifyResource, compactResources, isGenericMediaName, isSameDocumentPlaybackFallback, mergeResources, playerPlaybackResources, resourceFingerprint, resourceId, resourceMatchesPlaybackSource, resourceRank, visiblePlaybackResources, type MediaResource, type PlaybackContext } from '../lib/resources'
import { resourceQuality } from '../lib/hlsManifest'
import { THEME_BASE_CSS, THEME_STORAGE_KEY, THEME_TOKENS_CSS, applyTheme, normalizeThemePreference } from '../lib/theme'

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

export default defineContentScript({
  matches: ['<all_urls>'],
  allFrames: true,
  // All overlay/theme CSS is constructed below inside the isolated root. `ui`
  // makes WXT fetch a generated content.css even when no CSS bundle exists,
  // producing one denied chrome-extension request on every page.
  cssInjectionMode: 'manual',
  async main(ctx) {
    const eventVideo = (event: Event): HTMLVideoElement | null => {
      const path = typeof event.composedPath === 'function' ? event.composedPath() : []
      return (path.find(value => value instanceof HTMLVideoElement)
        || (event.target instanceof HTMLVideoElement ? event.target : null)) as HTMLVideoElement | null
    }
    let activateContent: (() => void) | null = null
    let activationRequested = false
    const activation = new Promise<void>(resolve => { activateContent = resolve })
    const requestActivation = () => {
      if (activationRequested) return
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
    document.addEventListener('play', rememberPendingPlayback, true)
    document.addEventListener('playing', rememberPendingPlayback, true)
    const resources = new Map<string, MediaResource>()
    // The UI mount below is asynchronous. Capture bridge/webRequest messages
    // during that window instead of losing the first manifest from a fast MSE
    // player (which otherwise leaves the overlay in “正在识别” forever).
    const earlyResourceEvents: Array<{ url: string; mimeType?: string }> = []
    const earlyMseEvents: Array<{ blobUrl: string; mediaUrl: string; purpose?: 'mse' | 'download' }> = []
    const earlyRuntimeMessages: any[] = []
    let contentReady = false
    const earlyResourceListener = (event: Event) => {
      if (contentReady) return
      const detail = (event as CustomEvent).detail || {}
      if (typeof detail.url === 'string') {
        earlyResourceEvents.push({ url: detail.url, mimeType: detail.mimeType })
        if (classifyResource(detail.url, detail.mimeType)) requestActivation()
      }
      if (earlyResourceEvents.length > 200) earlyResourceEvents.splice(0, earlyResourceEvents.length - 200)
    }
    const earlyMseListener = (event: Event) => {
      if (contentReady) return
      const detail = (event as CustomEvent).detail || {}
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
      if (contentReady || !['captured-resource', 'collect-selection', 'open-media-panel'].includes(String(message?.type || ''))) return
      if (message.type === 'captured-resource' && !message.resource?.url) return
      earlyRuntimeMessages.push(message)
      if (earlyRuntimeMessages.length > 100) earlyRuntimeMessages.splice(0, earlyRuntimeMessages.length - 100)
      requestActivation()
    }
    window.addEventListener('__hls_downloader_resource__', earlyResourceListener)
    window.addEventListener('__hls_downloader_mse__', earlyMseListener)
    browser.runtime.onMessage.addListener(earlyRuntimeListener)
    const activationRoots = new WeakSet<Document | ShadowRoot>()
    const activationMediaRoots: Array<Document | ShadowRoot> = []
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
      root.addEventListener('play', rememberPendingPlayback, true)
      root.addEventListener('playing', rememberPendingPlayback, true)
      if (root.querySelector('video,audio,source[src]')) {
        requestActivation()
      }
    }
    const earlyShadowListener = (event: Event) => {
      const host = event.composedPath().find(value => value instanceof Element) as Element | undefined
      if (host?.shadowRoot) observeActivationRoot(host.shadowRoot)
    }
    window.addEventListener('__hls_downloader_shadow__', earlyShadowListener, true)
    observeActivationRoot(document)
    // The page may create an open ShadowRoot from an inline document-start
    // script before this isolated-world bootstrap is scheduled. One bounded
    // initial scan closes that race without enabling the full DOM observer/UI.
    discoverActivationShadowRoots(document)
    // The bootstrap is ready for download-intent and background messages even
    // though the expensive media UI remains dormant until concrete evidence.
    document.documentElement.setAttribute('data-hls-downloader-extension', '1')
    await activation
    // Rendering is intentionally frequent on live pages. Keep handoff state
    // outside the DOM so a network event cannot replace an in-flight button.
    const resourceSendStates = new Map<string, { label: string, disabled: boolean }>()
    let activePlayback: PlaybackContext | null = null
    let activeVideo: HTMLVideoElement | null = null
    let selectionMode = false
    // A page can contain several players.  Keep playback evidence per element
    // instead of treating the latest network manifest as belonging to every
    // video in the document.
    let playbackByVideo = new WeakMap<HTMLVideoElement, PlaybackContext>()
    const knownVideos = new Set<HTMLVideoElement>()
    const currentVideos = () => {
      for (const video of knownVideos) {
        if (!video.isConnected) knownVideos.delete(video)
      }
      return [...knownVideos]
    }
    const mseEvidenceByBlob = new Map<string, { urls: Set<string>; seenAt: number }>()
    const replaceResources = (values: MediaResource[]) => {
      resources.clear()
      for (const value of values) resources.set(resourceFingerprint(value), value)
    }
    const resourcePresentationSignature = () => [...resources.values()]
      .map(resource => [
        resourceFingerprint(resource), resource.url, resource.title || '', resource.filename || '',
        resource.kind, resource.mimeType || '', resource.size || 0, resource.estimatedSize || 0,
        resource.width || 0, resource.height || 0, resource.bandwidth || 0, resource.duration || 0,
        resource.isLive === true ? 1 : resource.isLive === false ? 0 : '', resource.inspected ? 1 : 0,
        (resource.variants || []).map(variant => `${variant.url}:${variant.width || 0}:${variant.height || 0}:${variant.bandwidth || 0}`).join(','),
        (resource.playbackUrls || []).join(','), (resource.playbackPatterns || []).join(','),
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
          .wrap{display:none;position:fixed;z-index:2147483647;color:var(--text);filter:drop-shadow(0 6px 12px var(--shadow))}.wrap.open{display:block}
          .panel{display:none;width:min(344px,calc(100vw - 20px));max-height:min(520px,calc(100vh - 20px));background:var(--surface);border:1px solid var(--overlay-border);border-radius:9px;overflow:hidden}.open .panel{display:block}
          header{display:flex;align-items:center;justify-content:space-between;padding:7px 8px 7px 9px;border-bottom:1px solid var(--border);background:var(--surface-2);color:var(--text);font:600 12px system-ui;cursor:grab;touch-action:none}.title{display:flex;align-items:center;gap:6px}.title img{width:16px;height:16px;border-radius:4px}.head-actions{display:flex;align-items:center;gap:4px}
          .pin,.close{height:27px;border:0;border-radius:5px;background:var(--surface-3);color:var(--text);cursor:pointer}.pin{padding:0 8px;font:11px system-ui}.pin.active{background:color-mix(in srgb,var(--green) 18%,var(--surface-3));color:var(--green)}.close{display:grid;place-items:center;width:27px;font:700 18px/1 system-ui}.pin:hover,.close:hover{background:color-mix(in srgb,var(--primary) 14%,var(--surface-3))}.list{max-height:calc(min(520px,calc(100vh - 20px)) - 78px);overflow-y:auto;overscroll-behavior:contain}
          .item{padding:9px 10px;border-bottom:1px solid var(--border)}.item:last-child{border-bottom:0}.item:hover{background:var(--surface-2)}.meta{min-width:0}.name{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;font:600 12px/1.35 system-ui;overflow-wrap:anywhere;color:var(--text)}.kind{overflow:hidden;color:var(--muted);font:10.5px/1.35 system-ui;margin-top:3px;text-overflow:ellipsis;white-space:nowrap}.resource-url{display:block;margin-top:4px;color:var(--faint);font:10px/1.4 ui-monospace,SFMono-Regular,Consolas,monospace;overflow-wrap:anywhere;user-select:text}.quality-select{width:min(184px,100%);margin-top:6px}.item-actions{display:flex;gap:5px;margin-top:8px}.download{min-width:0;flex:1;height:29px;border:0;border-radius:6px;background:var(--primary);color:var(--on-primary);padding:4px 6px;cursor:pointer;font-weight:600;font-size:11px}.download:hover{background:var(--primary-hover)}.download[disabled]{cursor:default;opacity:.6}.download.push-tv{background:color-mix(in srgb,var(--purple) 75%,var(--surface))}.download.push-tv:hover{background:var(--purple)}.download.cast{background:color-mix(in srgb,var(--green) 78%,var(--surface))}.download.cast:hover{background:var(--green)}.result{padding:7px 10px;background:color-mix(in srgb,var(--green) 14%,var(--surface));color:var(--green);font:11px/1.4 system-ui}.result.error{background:color-mix(in srgb,var(--red) 12%,var(--surface));color:var(--red)}
          .video-buttons{position:fixed;inset:0;z-index:2147483646;pointer-events:none}.video-action-group{position:fixed;display:flex;align-items:center;gap:4px;pointer-events:auto}.video-download{position:relative;display:flex;align-items:center;gap:7px;height:34px;padding:0 12px;border:1px solid color-mix(in srgb,var(--primary) 60%,#fff 0%);border-radius:7px;background:var(--primary);color:var(--on-primary);box-shadow:0 3px 10px var(--shadow);cursor:grab;touch-action:none;user-select:none;-webkit-user-select:none;font:600 12px system-ui}.video-download:active{cursor:grabbing}.video-download:hover{background:var(--primary-hover)}.video-download.identifying{border-color:var(--overlay-border);background:color-mix(in srgb,var(--surface) 88%,var(--primary));color:var(--muted)}.video-download.identifying:hover{background:color-mix(in srgb,var(--surface) 88%,var(--primary))}.video-download img{width:18px;height:18px;border-radius:4px}.video-download b{display:inline-grid;place-items:center;min-width:18px;height:18px;padding:0 4px;border-radius:9px;background:rgba(255,255,255,.9);color:var(--primary);font:700 10px system-ui}.video-more{display:grid;width:34px;height:34px;place-items:center;border:1px solid color-mix(in srgb,var(--primary) 60%,#fff 0%);border-radius:7px;background:var(--surface);color:var(--primary);box-shadow:0 3px 10px var(--shadow);cursor:pointer;font:700 18px/1 system-ui}.video-more:hover{background:var(--surface-2)}
          button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}@media(prefers-reduced-motion:reduce){*{transition:none!important}}
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
        const wrap = root.querySelector<HTMLElement>('.wrap')!
        return root
      },
    })
    ui.mount()
    const wrap = ui.shadow.querySelector<HTMLElement>('.wrap')
    const themeRoot = wrap?.parentElement
    if (themeRoot) {
      let removeTheme = applyTheme(themeRoot, 'auto')
      void browser.storage.local.get(THEME_STORAGE_KEY).then(stored => {
        removeTheme()
        removeTheme = applyTheme(themeRoot, normalizeThemePreference(stored[THEME_STORAGE_KEY]))
      }).catch(() => {})
      browser.storage.onChanged.addListener((changes, area) => {
        if (area !== 'local' || !changes[THEME_STORAGE_KEY]) return
        removeTheme()
        removeTheme = applyTheme(themeRoot, normalizeThemePreference(changes[THEME_STORAGE_KEY].newValue))
      })
    }
    const dragHandles = ui.shadow.querySelectorAll<HTMLElement>('header')
    let dragged = false
    let pinned = false
    let panelPosition: { x: number, y: number } | null = null
    let videoButtonPositions = new WeakMap<HTMLVideoElement, { x: number, y: number }>()
    let videoControlDragging = false
    let collapseTimer: ReturnType<typeof setTimeout> | null = null
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
      wrap?.classList.toggle('open', open)
      if (open) requestAnimationFrame(fitPanel)
    }
    const pinButton = ui.shadow.querySelector<HTMLButtonElement>('.pin')
    const setPinned = (value: boolean) => {
      pinned = value
      pinButton?.classList.toggle('active', value)
      if (pinButton) pinButton.textContent = value ? '已固定' : '固定'
      if (value) setOpen(true)
    }
    wrap?.addEventListener('mouseenter', () => {
      if (collapseTimer) clearTimeout(collapseTimer)
      setOpen(true)
    })
    wrap?.addEventListener('mouseleave', () => {
      if (pinned || dragged) return
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
    void browser.storage.local.remove(['panelPinned', 'panelPosition', 'videoButtonPosition'])
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
        handle.releasePointerCapture?.(event.pointerId)
        window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', finish)
        panelPosition = { x: wrap.offsetLeft, y: wrap.offsetTop }
        setTimeout(() => { dragged = false }, 0)
      }
      window.addEventListener('pointermove', move); window.addEventListener('pointerup', finish, { once: true })
    }))
    window.addEventListener('resize', fitPanel)

    const applySendState = (resource: MediaResource, button: HTMLButtonElement, fallbackLabel: string, action: OverlayAction = 'download') => {
      const state = resourceSendStates.get(overlaySendKey(resourceFingerprint(resource), action))
      const label = button.querySelector<HTMLElement>('.download-label')
      if (label) label.textContent = state?.label || fallbackLabel
      else button.textContent = state?.label || fallbackLabel
      if (state?.disabled) button.setAttribute('disabled', '')
      else button.removeAttribute('disabled')
    }

    const setSendState = (resource: MediaResource, button: HTMLButtonElement, label: string, disabled: boolean, fallbackLabel = '下载', action: OverlayAction = 'download') => {
      resourceSendStates.set(overlaySendKey(resourceFingerprint(resource), action), { label, disabled })
      applySendState(resource, button, fallbackLabel, action)
    }

    const sendResource = (resource: MediaResource, button: HTMLButtonElement) => {
      const key = resourceFingerprint(resource)
      if (resourceSendStates.get(key)?.disabled) return
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      setSendState(resource, button, '发送中', true)
      void runtimeMessage({ type: 'offer', resource }).then(async response => {
        if (!response?.ok || !response?.handoff?.id) throw new Error(response?.error || '桌面端未接受请求')
        setSendState(resource, button, '等待确认', true)
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `请在桌面下载器确认：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
        const handoffId = response.handoff.id
        const deadline = Date.now() + 130_000
        while (Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 1000))
          const statusResponse = await runtimeMessage({ type: 'handoff-status', handoffId }).catch(() => null)
          const handoff = statusResponse?.handoff || statusResponse
          const status = String(handoff?.status || '')
          if (!status || status === 'pending' || status === 'accepting') continue
          if (status === 'connection_lost') {
            if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = '桌面端短暂断开，正在自动恢复确认状态' }
            continue
          }
          if (status === 'accepted') {
            setSendState(resource, button, '已加入', true)
            if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `已加入下载队列：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
            setTimeout(() => resourceSendStates.delete(key), 2_500)
          } else {
            setSendState(resource, button, status === 'expired' ? '已过期' : '重试', false)
            if (result) { result.hidden = false; result.classList.add('error'); result.textContent = status === 'canceled' || status === 'rejected' ? '已取消下载确认' : `确认已${status}` }
          }
          return
        }
        setSendState(resource, button, '重试', false)
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = '桌面端确认超时，请重试' }
      }).catch(reason => {
        setSendState(resource, button, '重试', false)
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '发送失败' }
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
          if (['done', 'failed', 'canceled'].includes(String(status?.status || ''))) return status
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
        setTimeout(() => resourceSendStates.delete(key), 2_000)
      }).catch(reason => {
        setSendState(resource, button, '重试', false, overlayActionFallback('tvbox'), 'tvbox')
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
          if (['done', 'failed', 'canceled'].includes(String(status?.status || ''))) break
        }
        if (status?.status !== 'done') throw new Error(status?.message || '投屏未完成')
        setSendState(resource, button, '已发送', true, overlayActionFallback('cast'), 'cast')
        if (result) result.textContent = status.message || '投屏成功'
        setTimeout(() => resourceSendStates.delete(key), 2_000)
      }).catch(reason => {
        setSendState(resource, button, '重试', false, overlayActionFallback('cast'), 'cast')
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '投屏请求失败' }
      })
    }

    const openVideoPanel = (rect: DOMRect) => {
      if (!wrap) return
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
      const layer = ui.shadow.querySelector<HTMLElement>('.video-buttons')
      if (!layer) return
      // Players emit timeupdate while the pointer is down. Replacing the
      // control in that interval cancels pointer capture before it can move.
      if (videoControlDragging) return
      layer.replaceChildren()
      let visible = 0
      const videos = currentVideos()
        .map(video => ({ video, rect: video.getBoundingClientRect(), playback: playbackByVideo.get(video) || null }))
        .filter(({ video, rect, playback }) => Boolean(playback)
          && (video === activeVideo || (!video.paused && !video.ended))
          && rect.width >= 180 && rect.height >= 100
          && rect.bottom >= 0 && rect.top <= innerHeight && rect.right >= 0 && rect.left <= innerWidth)
        .sort((left, right) => Number(right.video === activeVideo) - Number(left.video === activeVideo)
          || right.rect.width * right.rect.height - left.rect.width * left.rect.height)
      const activeMseVideos = videos.filter(({ video, playback }) => Boolean(playback?.sourceUrls.some(source => source.startsWith('blob:')))
        && (video === activeVideo || !video.paused))
      // Render beside every real, played video that has its own evidence.  A
      // blob/MSE source cannot name its manifest; when two MSE players are
      // active in this frame, page-level manifests are ambiguous and are not
      // shown beside either player.
      videos.forEach(({ video, rect, playback }) => {
        if (!playback) return
        const sourceUrls = playback.sourceUrls
        const candidates = playerPlaybackResources(
          [...resources.values()],
          playback,
          activeMseVideos.length,
          8,
        )
        const exact = candidates.filter(item => sourceUrls.some(source => resourceMatchesPlaybackSource(item, source)))
        const hasExactPlayerMatch = exact.length > 0
        const choices = (hasExactPlayerMatch ? exact : candidates)
          .sort((left, right) => resourceRank(right) - resourceRank(left) || (right.height || 0) - (left.height || 0) || (right.bandwidth || 0) - (left.bandwidth || 0) || (right.size || right.estimatedSize || 0) - (left.size || left.estimatedSize || 0))
        if (!shouldShowMediaOverlay({ hasPlayback: true, hasActiveVideo: true, resourceCount: choices.length })) return
        const identifying = choices.length === 0
        // playerPlaybackResources already enforces the ambiguity boundary for
        // blob/MSE (per-SourceBuffer evidence, or a sole active MSE session).
        // Requiring a literal currentSrc match again made a uniquely resolved
        // MSE video open a pointless one-item picker.
        const oneClickChoice = choices.length === 1
        visible += 1
        const buttonWidth = 156
        const buttonHeight = 34
        const controlsWidth = buttonWidth + (oneClickChoice ? 38 : 0)
        const group = document.createElement('div')
        group.className = 'video-action-group'
        const button = document.createElement('button')
        button.type = 'button'; button.className = `video-download${identifying ? ' identifying' : ''}`
        if (oneClickChoice) button.dataset.resourceId = choices[0].id
        button.title = identifying ? '正在识别当前播放的视频资源' : oneClickChoice ? '下载当前视频' : '选择当前页面检测到的视频资源'
        if (identifying) button.setAttribute('aria-disabled', 'true')
        const besidePlayer = rect.right + 8
        const defaultLeft = besidePlayer + controlsWidth <= innerWidth - 8
          ? besidePlayer
          : Math.max(8, Math.min(rect.right - controlsWidth - 8, innerWidth - controlsWidth - 8))
        const defaultTop = Math.max(8, Math.min(rect.top + 8, innerHeight - buttonHeight - 8))
        const saved = videoButtonPositions.get(video)
        group.style.left = `${saved ? Math.max(8, Math.min(saved.x, innerWidth - controlsWidth - 8)) : defaultLeft}px`
        group.style.top = `${saved ? Math.max(8, Math.min(saved.y, innerHeight - buttonHeight - 8)) : defaultTop}px`
        const icon = document.createElement('img'); icon.src = browser.runtime.getURL('/icon-32.png'); icon.alt = ''
         const fallbackLabel = identifying ? '正在识别' : oneClickChoice ? '下载视频' : '选择资源'
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
          const startX = event.clientX; const startY = event.clientY
          const startLeft = group.offsetLeft; const startTop = group.offsetTop
          const move = (next: PointerEvent) => {
            if (next.pointerId !== event.pointerId) return
            videoDragged ||= Math.abs(next.clientX - startX) + Math.abs(next.clientY - startY) > 4
            const height = button.offsetHeight || buttonHeight
            group.style.left = `${Math.max(8, Math.min(innerWidth - controlsWidth - 8, startLeft + next.clientX - startX))}px`
            group.style.top = `${Math.max(8, Math.min(innerHeight - height - 8, startTop + next.clientY - startY))}px`
          }
          const finish = (next: PointerEvent) => {
            if (next.pointerId !== event.pointerId) return
            button.releasePointerCapture?.(event.pointerId)
            window.removeEventListener('pointermove', move, true)
            window.removeEventListener('pointerup', finish, true)
            window.removeEventListener('pointercancel', cancel, true)
            if (videoDragged) {
              videoButtonPositions.set(video, { x: group.offsetLeft, y: group.offsetTop })
            }
            videoControlDragging = false
            scheduleVideoButtons()
          }
          const cancel = (next: PointerEvent) => {
            if (next.pointerId !== event.pointerId) return
            button.releasePointerCapture?.(event.pointerId)
            window.removeEventListener('pointermove', move, true)
            window.removeEventListener('pointerup', finish, true)
            window.removeEventListener('pointercancel', cancel, true)
            videoControlDragging = false
            scheduleVideoButtons()
          }
          // Use the window capture phase as a fallback for players that stop
          // dispatching pointer events from their overlay while the pointer is
          // moved outside the button. Pointer capture alone is not reliable in
          // every iframe/player combination.
          window.addEventListener('pointermove', move, true)
          window.addEventListener('pointerup', finish, true)
          window.addEventListener('pointercancel', cancel, true)
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
             return
           }
           if (oneClickChoice) {
             event.preventDefault()
             event.stopImmediatePropagation()
             sendResource(choices[0], button)
             return
           }
          // Ambiguous MSE resources still require an evidence selection panel.
          openVideoPanel(rect)
        })
        if (choices[0]) applySendState(choices[0], button, fallbackLabel)
        group.append(button)
        if (oneClickChoice) {
          const more = document.createElement('button')
          more.type = 'button'; more.className = 'video-more'; more.textContent = '⋯'
          more.title = '更多操作：投屏或推送当前媒体链接'
          more.setAttribute('aria-label', '更多操作：投屏或推送当前媒体链接')
          more.addEventListener('pointerdown', event => event.stopPropagation())
          more.addEventListener('click', event => {
            event.preventDefault(); event.stopPropagation()
            openVideoPanel(rect)
          })
          group.append(more)
        }
        layer.append(group)
      })
      if (!visible) {
        if (pinned) setPinned(false)
        setOpen(false)
        panelPosition = null
      }
    }

    const render = () => {
      const list = ui.shadow.querySelector('.list')
      if (!list) return
      const entries = selectionMode && !activePlayback
        ? compactResources([...resources.values()], 40).slice(0, 20)
        : visiblePlaybackResources([...resources.values()], activePlayback, 8)
      list.replaceChildren()
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
        const sizeLabel = resource.size ? formatSize(resource.size) : likelySize ? `约 ${formatSize(likelySize)}` : '大小未知'
        const kind = document.createElement('div'); kind.className = 'kind'; kind.textContent = [resource.kind.toUpperCase(), streamMode, quality, resource.width && resource.height ? `${resource.width}×${resource.height}` : '', bandwidth, duration, sizeLabel, host].filter(Boolean).join(' · ')
        const resourceUrl = document.createElement('code'); resourceUrl.className = 'resource-url'; resourceUrl.title = resource.url; resourceUrl.textContent = resource.url
        let selected = resource
        if (resource.variants?.length) {
          const select = document.createElement('select')
          select.className = 'quality-select'
          select.setAttribute('aria-label', '选择视频清晰度')
          const automatic = document.createElement('option')
          automatic.value = resource.url
          automatic.textContent = '自动（最高）'
          select.append(automatic)
          resource.variants.forEach(variant => {
            const option = document.createElement('option')
            option.value = variant.url
            option.textContent = [variant.quality || (variant.height ? `${variant.height}p` : '线路'), variant.bandwidth ? `${(variant.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''].filter(Boolean).join(' · ')
            select.append(option)
          })
           select.addEventListener('change', () => {
            const variant = resource.variants?.find(item => item.url === select.value)
            selected = variant ? { ...resource, ...variant, url: variant.url, variants: undefined } : resource
            applySendState(selected, button, '下载')
          })
          meta.append(name, kind, resourceUrl, select)
        } else {
          meta.append(name, kind, resourceUrl)
        }
        const actions = document.createElement('div'); actions.className = 'item-actions'
         const button = document.createElement('button'); button.className = 'download'; button.textContent = '下载'
         button.addEventListener('click', () => sendResource(selected, button))
         applySendState(selected, button, '下载')
        const pushButton = document.createElement('button'); pushButton.className = 'download push-tv'; pushButton.textContent = '推送链接'
        pushButton.title = '直接推送当前媒体链接到 TVBox'
        pushButton.addEventListener('click', () => pushToTv(selected, pushButton))
        applySendState(selected, pushButton, overlayActionFallback('tvbox'), 'tvbox')
        const castButton = document.createElement('button'); castButton.className = 'download cast'; castButton.textContent = '投屏链接'
        castButton.title = '直接投屏当前媒体链接到 DLNA 或 Chromecast'
        castButton.addEventListener('click', () => castResource(selected, castButton))
        applySendState(selected, castButton, overlayActionFallback('cast'), 'cast')
        actions.append(button, pushButton, castButton)
        row.append(meta, actions); list.append(row)
      })
      updateVideoButtons()
    }
    let renderFrame = 0
    const scheduleRender = () => {
      if (renderFrame) return
      renderFrame = requestAnimationFrame(() => {
        renderFrame = 0
        render()
      })
    }
    let positionFrame = 0
    const scheduleVideoButtons = () => {
      if (positionFrame) return
      positionFrame = requestAnimationFrame(() => { positionFrame = 0; updateVideoButtons() })
    }
    window.addEventListener('scroll', scheduleVideoButtons, { capture: true, passive: true })
    window.addEventListener('resize', scheduleVideoButtons)
    document.addEventListener('fullscreenchange', scheduleVideoButtons)

    const observedVideoSizes = new WeakSet<HTMLVideoElement>()
    const videoResizeObserver = typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver(() => scheduleVideoButtons())
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
      return [
        ...(current && !currentIsDocumentFallback ? [{ url: current, mimeType: '' }] : []),
        ...declared,
      ]
    }
    const add = (url: string, mimeType = '', playbackSource = false) => {
      const kind = playbackSource ? classifyPlaybackSource(url, mimeType) : classifyResource(url, mimeType)
      if (!kind) return
      let filename = ''
      try { filename = decodeURIComponent(new URL(url).pathname.split('/').pop() || '') } catch {}
      const resource = { id: resourceId(url), url, kind, mimeType, pageUrl: location.href, title: pageMediaTitle() || filename, filename, seenAt: Date.now() }
      const changed = addResource(resource)
      if (changed) scheduleRender()
      void runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
    }

    const markVideoPlayback = (video: HTMLVideoElement, eventType: string) => {
      const rect = video.getBoundingClientRect()
      if (rect.width < 180 || rect.height < 100 || rect.bottom < 0 || rect.top > innerHeight || rect.right < 0 || rect.left > innerWidth) return
      const previousActiveVideo = activeVideo
      const explicitActivation = ['play', 'playing', 'initializing'].includes(eventType)
      // timeupdate fires several times per second for every playing element.
      // It may refresh source evidence, but must not continuously steal the
      // active-player identity or rebuild the complete overlay DOM.
      if (explicitActivation || !activeVideo || !activeVideo.isConnected || activeVideo.paused || activeVideo.ended) {
        activeVideo = video
        selectionMode = false
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
        directSources.forEach(source => {
          if (!source.url || seenDirectSources.has(source.url)) return
          seenDirectSources.add(source.url)
          add(source.url, source.mimeType, true)
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
      mediaEvents.forEach(type => root.addEventListener(type, markPlayback, true))
      new MutationObserver(mutations => {
        let playbackChanged = false
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
      }).observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] })
    }
    window.addEventListener('__hls_downloader_shadow__', event => {
      const host = event.composedPath().find(value => value instanceof Element) as Element | undefined
      if (host?.shadowRoot) {
        registerMediaRoot(host.shadowRoot)
        queueMicrotask(syncPlayingVideos)
      }
    }, true)
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
      const detail = (event as CustomEvent).detail || {}
      add(detail.url, detail.mimeType)
    }
    const handleMseEvent = (event: Event) => {
      const detail = (event as CustomEvent).detail || {}
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
          mseResourceUrls: [...new Set([...(playback.mseResourceUrls || []), mediaUrl])],
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
        add(mediaUrl)
        scheduleRender()
      }
    }
    window.removeEventListener('__hls_downloader_resource__', earlyResourceListener)
    window.removeEventListener('__hls_downloader_mse__', earlyMseListener)
    window.addEventListener('__hls_downloader_resource__', handleResourceEvent)
    window.addEventListener('__hls_downloader_mse__', handleMseEvent)
    // Ask the MAIN-world hook to replay events emitted before this content
    // script/UI finished mounting (especially fast MSE and preloaded HLS).
    window.dispatchEvent(new Event('__hls_downloader_replay__'))
    earlyResourceEvents.splice(0).forEach(event => add(event.url, event.mimeType))
    earlyMseEvents.splice(0).forEach(event => handleMseEvent(new CustomEvent('__hls_downloader_mse__', { detail: event })))
    document.querySelectorAll<HTMLVideoElement | HTMLAudioElement>('video,audio').forEach(media => {
      mediaElementSources(media).forEach(source => add(source.url, source.mimeType, true))
    })
    document.querySelectorAll<HTMLSourceElement>('source').forEach(source => {
      const url = explicitElementSource(source)
      if (url) add(url, source.type || '', true)
    })
    new PerformanceObserver(list => list.getEntries().forEach(entry => add(entry.name))).observe({ type: 'resource', buffered: true })
    const handleRuntimeMessage = (message: any) => {
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
        selectionMode = true
        activePlayback = null
        activeVideo = null
        root.querySelectorAll<HTMLAnchorElement>('a[href]').forEach(anchor => add(anchor.href))
        setOpen(true)
        render()
      }
      if (message?.type === 'open-media-panel') {
        if (!activePlayback) selectionMode = true
        setOpen(true)
        render()
      }
    }
    browser.runtime.onMessage.addListener(handleRuntimeMessage)
    contentReady = true
    browser.runtime.onMessage.removeListener(earlyRuntimeListener)
    earlyRuntimeMessages.splice(0).forEach(handleRuntimeMessage)
    let currentPageUrl = pageKey(location.href)
    const loadPageResources = (pageUrl: string) => {
      void runtimeMessage({ type: 'list', pageUrl }).then((stored: MediaResource[]) => {
        if (!Array.isArray(stored) || pageKey(location.href) !== pageKey(pageUrl)) return
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
    window.addEventListener('popstate', syncPage)
    window.addEventListener('hashchange', syncPage)
    window.addEventListener('__hls_downloader_navigation__', syncPage)
    window.addEventListener('pageshow', () => { syncPage(); syncPlayingVideos() })
    document.addEventListener('visibilitychange', () => {
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

function formatSize(size: number): string {
  if (!Number.isFinite(size) || size <= 0) return '大小未知'
  const units = ['B', 'KB', 'MB', 'GB']
  let value = size; let index = 0
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index += 1 }
  return `${value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`
}

function formatDuration(seconds: number): string {
  const rounded = Math.round(seconds)
  const hours = Math.floor(rounded / 3600)
  const minutes = Math.floor((rounded % 3600) / 60)
  const remaining = rounded % 60
  return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${String(remaining).padStart(2, '0')}` : `${minutes}:${String(remaining).padStart(2, '0')}`
}
