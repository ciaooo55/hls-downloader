import { browser } from 'wxt/browser'
import { classifyResource, isGenericMediaName, mergeResources, resourceFingerprint, resourceId, visiblePlaybackResources, type MediaResource, type PlaybackContext } from '../lib/resources'
import { resourceQuality } from '../lib/hlsManifest'

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
  cssInjectionMode: 'ui',
  async main(ctx) {
    document.documentElement.setAttribute('data-hls-downloader-extension', '1')
    const resources = new Map<string, MediaResource>()
    let activePlayback: PlaybackContext | null = null
    const replaceResources = (values: MediaResource[]) => {
      resources.clear()
      for (const value of values) resources.set(resourceFingerprint(value), value)
    }
    const addResource = (resource: MediaResource) => {
      replaceResources(mergeResources([...resources.values()], resource, 40))
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
          .wrap{position:fixed;right:14px;top:35%;z-index:2147483647;color:#102a3a;filter:drop-shadow(0 5px 8px #07598529)}
          .toggle{display:grid;place-items:center;width:34px;height:34px;padding:2px;border:1.5px solid #38bdf8;border-radius:9px;background:#f0fbff;cursor:pointer}.toggle img{width:24px;height:24px;border-radius:5px}
          .panel{display:none;width:min(420px,calc(100vw - 20px));max-height:70vh;background:#fff;border:1px solid #bae6fd;border-radius:9px;overflow:hidden}.open .panel{display:block}.open .toggle{display:none}
          header{display:flex;align-items:center;justify-content:space-between;padding:8px 9px 8px 10px;border-bottom:1px solid #dff5ff;background:#f0fbff;font:600 13px system-ui}.title{display:flex;align-items:center;gap:6px}.title img{width:16px;height:16px;border-radius:4px}.head-actions{display:flex;align-items:center;gap:5px}
          .pin,.close{height:30px;border:0;border-radius:5px;background:#e0f2fe;color:#075985;cursor:pointer}.pin{padding:0 9px;font:12px system-ui}.pin.active{background:#d1fae5;color:#047857}.close{display:grid;place-items:center;width:30px;font:700 20px/1 system-ui}.list{overflow:auto;max-height:58vh}.empty{padding:20px;color:#526b79;font:13px system-ui}
          .item{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:10px;padding:12px;border-bottom:1px solid #e7f4f8}.item:hover{background:#f7fcff}.meta{min-width:0}.name{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;font:600 13px/1.35 system-ui;overflow-wrap:anywhere}.url{display:-webkit-box;overflow:hidden;-webkit-line-clamp:1;-webkit-box-orient:vertical;color:#54717f;font:11px/1.35 system-ui;margin-top:4px;overflow-wrap:anywhere}.item:hover .name,.item:hover .url{-webkit-line-clamp:unset}.kind{color:#25627b;font:12px system-ui;margin-top:4px}.quality-select{width:min(190px,100%);height:27px;margin-top:6px;border:1px solid #bae6fd;border-radius:5px;background:#f0fbff;color:#075985;padding:0 6px;font:11px system-ui}.item-actions{display:flex;flex-direction:column;gap:5px;align-self:center}.download{min-width:58px;height:30px;border:0;border-radius:6px;background:#0ea5e9;color:white;padding:5px 10px;cursor:pointer;font-weight:600;font-size:12px}.download:hover{background:#0284c7}.download[disabled]{cursor:default;opacity:.65}.download.push-tv{background:#6366f1}.download.push-tv:hover{background:#4f46e5}.result{padding:8px 12px;background:#ecfdf5;color:#047857;font:12px/1.4 system-ui}.result.error{background:#fff1f2;color:#be123c}
          .video-buttons{position:fixed;inset:0;z-index:2147483646;pointer-events:none}.video-download{position:fixed;display:flex;align-items:center;gap:7px;height:34px;padding:0 12px;border:1px solid #38bdf8;border-radius:7px;background:#075985;color:#fff;box-shadow:0 3px 8px #00131f66;pointer-events:auto;cursor:pointer;font:600 12px system-ui}.video-download:hover{background:#0369a1}.video-download img{width:18px;height:18px;border-radius:4px}.video-download b{display:inline-grid;place-items:center;min-width:18px;height:18px;padding:0 4px;border-radius:9px;background:#e0f2fe;color:#075985;font:700 10px system-ui}
          button:focus-visible{outline:2px solid #0369a1;outline-offset:2px}@media(prefers-reduced-motion:reduce){*{transition:none!important}}
        `
        const image = () => {
          const icon = element('img') as HTMLImageElement
          icon.src = iconUrl
          icon.alt = ''
          return icon
        }
        const panelWrap = element('div', 'wrap')
        const toggle = element('button', 'toggle') as HTMLButtonElement
        toggle.type = 'button'
        toggle.title = '媒体嗅探：悬停展开'
        toggle.setAttribute('aria-label', '展开媒体嗅探')
        toggle.append(image())
        const panel = element('div', 'panel')
        const header = element('header')
        const title = element('span', 'title', '检测到的媒体')
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
        list.append(element('div', 'empty', '播放视频后会显示资源'))
        panel.append(header, result, list)
        panelWrap.append(toggle, panel)
        const videoButtons = element('div', 'video-buttons')
        root.append(style, panelWrap, videoButtons)
        container.append(root)
        const wrap = root.querySelector<HTMLElement>('.wrap')!
        root.querySelector('.toggle')!.addEventListener('click', () => {
          wrap.classList.add('open')
          const rect = wrap.getBoundingClientRect()
          wrap.style.left = `${Math.max(10, Math.min(rect.left, innerWidth - rect.width - 10))}px`
          wrap.style.top = `${Math.max(10, Math.min(rect.top, innerHeight - rect.height - 10))}px`
          wrap.style.right = 'auto'
        })
        return root
      },
    })
    ui.mount()
    const wrap = ui.shadow.querySelector<HTMLElement>('.wrap')
    const dragHandles = ui.shadow.querySelectorAll<HTMLElement>('.toggle, header')
    let dragged = false
    let pinned = false
    let collapseTimer: ReturnType<typeof setTimeout> | null = null
    const fitPanel = () => {
      if (!wrap) return
      const rect = wrap.getBoundingClientRect()
      if (rect.right > innerWidth - 10 || rect.bottom > innerHeight - 10 || rect.left < 10 || rect.top < 10) {
        wrap.style.left = `${Math.max(10, Math.min(rect.left, innerWidth - rect.width - 10))}px`
        wrap.style.top = `${Math.max(10, Math.min(rect.top, innerHeight - rect.height - 10))}px`
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
      void browser.storage.local.set({ panelPinned: value })
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
    void browser.storage.local.get(['panelPosition', 'panelPinned']).then(value => {
      const position = value.panelPosition as { x?: unknown; y?: unknown } | undefined
      pinned = value.panelPinned === true
      pinButton?.classList.toggle('active', pinned)
      if (pinButton) pinButton.textContent = pinned ? '已固定' : '固定'
      if (pinned) setOpen(true)
      if (wrap && position && typeof position.x === 'number' && typeof position.y === 'number'
        && Number.isFinite(position.x) && Number.isFinite(position.y)) {
        wrap.style.left = `${Math.max(0, position.x)}px`; wrap.style.top = `${Math.max(0, position.y)}px`; wrap.style.right = 'auto'
      }
    })
    dragHandles.forEach(handle => handle.addEventListener('pointerdown', event => {
      if (!wrap || (event.target as HTMLElement).closest('.close, .pin')) return
      dragged = false
      const startX = event.clientX; const startY = event.clientY
      const rect = wrap.getBoundingClientRect(); const startLeft = rect.left; const startTop = rect.top
      const move = (next: PointerEvent) => {
        if (Math.abs(next.clientX - startX) + Math.abs(next.clientY - startY) > 4) dragged = true
        wrap.style.left = `${Math.max(0, Math.min(innerWidth - 34, startLeft + next.clientX - startX))}px`
        wrap.style.top = `${Math.max(0, Math.min(innerHeight - 34, startTop + next.clientY - startY))}px`
        wrap.style.right = 'auto'
      }
      const finish = () => {
        window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', finish)
        void browser.storage.local.set({ panelPosition: { x: wrap.offsetLeft, y: wrap.offsetTop } })
        setTimeout(() => { dragged = false }, 0)
      }
      window.addEventListener('pointermove', move); window.addEventListener('pointerup', finish, { once: true })
    }))
    ui.shadow.querySelector('.toggle')?.addEventListener('click', event => {
      if (dragged) event.stopImmediatePropagation()
    }, true)
    window.addEventListener('resize', fitPanel)

    const sendResource = (resource: MediaResource, button: HTMLButtonElement) => {
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      button.setAttribute('disabled', ''); button.textContent = '发送中'
      void runtimeMessage({ type: 'offer', resource }).then(async response => {
        if (!response?.ok || !response?.handoff?.id) throw new Error(response?.error || '桌面端未接受请求')
        button.textContent = '待确认'
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `请在桌面下载器确认：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
        const handoffId = response.handoff.id
        const deadline = Date.now() + 130_000
        while (Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 1000))
          const statusResponse = await runtimeMessage({ type: 'handoff-status', handoffId }).catch(() => null)
          const handoff = statusResponse?.handoff || statusResponse
          const status = String(handoff?.status || '')
          if (!status || status === 'pending' || status === 'accepting') continue
          if (status === 'accepted') {
            button.textContent = '已加入'
            if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `已加入下载队列：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
          } else {
            button.removeAttribute('disabled')
            button.textContent = status === 'expired' ? '已过期' : '重试'
            if (result) { result.hidden = false; result.classList.add('error'); result.textContent = status === 'canceled' || status === 'rejected' ? '已取消下载确认' : `确认已${status}` }
          }
          return
        }
        button.removeAttribute('disabled'); button.textContent = '重试'
      }).catch(reason => {
        button.removeAttribute('disabled'); button.textContent = '重试'
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '发送失败' }
      })
    }

    const pushToTv = (resource: MediaResource, button: HTMLButtonElement) => {
      const name = resource.filename || resource.title || resource.kind.toUpperCase()
      if (!window.confirm(`确认推送到电视？\n\n${name}\n\n电视将直接打开该媒体地址。`)) return
      const result = ui.shadow.querySelector<HTMLElement>('.result')
      button.setAttribute('disabled', ''); button.textContent = '推送中'
      void runtimeMessage({ type: 'push-to-tv', resource }).then(response => {
        if (!response?.ok) throw new Error(response?.error || '电视推送失败')
        button.textContent = '已推送'
        if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `已推送到电视：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
      }).catch(reason => {
        button.removeAttribute('disabled'); button.textContent = '推电视'
        if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '推送失败' }
      }).finally(() => {
        setTimeout(() => { if (button.textContent === '已推送') { button.textContent = '推电视' } }, 2000)
      })
    }

    const updateVideoButtons = () => {
      const layer = ui.shadow.querySelector<HTMLElement>('.video-buttons')
      const toggle = ui.shadow.querySelector<HTMLButtonElement>('.toggle')
      if (!layer) return
      layer.replaceChildren()
      const entries = visiblePlaybackResources([...resources.values()], activePlayback, 8)
      let visible = 0
      const videos = [...document.querySelectorAll<HTMLVideoElement>('video')]
        .map(video => ({ video, rect: video.getBoundingClientRect() }))
        .filter(({ rect }) => rect.width >= 180 && rect.height >= 100 && rect.bottom >= 0 && rect.top <= innerHeight && rect.right >= 0 && rect.left <= innerWidth)
        .sort((left, right) => right.rect.width * right.rect.height - left.rect.width * left.rect.height)
      // IDM and AB place one action beside the dominant player. Showing a
      // duplicate button on every preview/advert video is noisy and ambiguous.
      videos.slice(0, 1).forEach(({ video, rect }) => {
        const sourceUrls = [video.currentSrc, video.src, ...[...video.querySelectorAll<HTMLSourceElement>('source[src]')].map(source => source.src)].filter(Boolean)
        const exact = entries.filter(item => sourceUrls.includes(item.url))
        const hasExactPlayerMatch = exact.length > 0
        const choices = hasExactPlayerMatch ? exact : entries
        if (!choices.length) return
        visible += 1
        const button = document.createElement('button')
        button.type = 'button'; button.className = 'video-download'; button.title = hasExactPlayerMatch && choices.length === 1 ? '使用 HLS Downloader 下载此视频' : '选择当前页面检测到的视频资源'
        button.style.left = `${Math.max(8, rect.right - 132)}px`; button.style.top = `${Math.max(8, rect.top + 8)}px`
        const icon = document.createElement('img'); icon.src = browser.runtime.getURL('/icon-32.png'); icon.alt = ''
        const label = document.createElement('span'); label.textContent = hasExactPlayerMatch && choices.length === 1 ? '下载视频' : '选择资源'
        button.append(icon, label)
        if (choices.length > 1) { const count = document.createElement('b'); count.textContent = String(choices.length); button.append(count) }
        button.addEventListener('click', () => {
          // MSE/blob players do not expose the actual manifest as currentSrc.
          // Never turn an unrelated single network entry into a one-click
          // download: opening the chooser lets the user see the evidence first.
          if (hasExactPlayerMatch && choices.length === 1) { sendResource(choices[0], button); return }
          if (wrap) {
            wrap.style.left = `${Math.max(10, Math.min(rect.right - 420, innerWidth - 430))}px`
            wrap.style.top = `${Math.max(10, Math.min(rect.top + 50, innerHeight - 420))}px`
            wrap.style.right = 'auto'; setOpen(true)
          }
        })
        layer.append(button)
      })
      if (toggle) toggle.hidden = visible > 0
    }

    const render = () => {
      const list = ui.shadow.querySelector('.list')
      if (!list) return
      const entries = visiblePlaybackResources([...resources.values()], activePlayback, 8)
      list.replaceChildren()
      if (!entries.length) {
        const empty = document.createElement('div')
        empty.className = 'empty'
        empty.textContent = activePlayback ? '未找到与本次播放关联的可下载资源' : '请先播放主视频，再显示关联资源'
        list.append(empty)
      }
      entries.forEach(resource => {
        const row = document.createElement('div'); row.className = 'item'
        const meta = document.createElement('div'); meta.className = 'meta'
        const name = document.createElement('div'); name.className = 'name'; name.title = resource.title || resource.filename || resource.url; name.textContent = resource.title || resource.filename || resource.url.split('/').pop() || resource.url
        let host = ''; try { host = new URL(resource.url).host } catch {}
        const quality = resource.quality || resourceQuality(resource.url, resource.height)
        const duration = resource.duration ? formatDuration(resource.duration) : ''
        const bandwidth = resource.bandwidth ? `${(resource.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''
        const likelySize = resource.size || resource.estimatedSize || 0
        const sizeLabel = resource.size ? formatSize(resource.size) : likelySize ? `约 ${formatSize(likelySize)}` : '大小未知'
        const kind = document.createElement('div'); kind.className = 'kind'; kind.textContent = [resource.kind.toUpperCase(), quality, resource.width && resource.height ? `${resource.width}×${resource.height}` : '', bandwidth, duration, sizeLabel, host].filter(Boolean).join(' · ')
        const url = document.createElement('div'); url.className = 'url'; url.title = resource.url; url.textContent = resource.url
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
          })
          meta.append(name, kind, select, url)
        } else {
          meta.append(name, kind, url)
        }
        const actions = document.createElement('div'); actions.className = 'item-actions'
        const button = document.createElement('button'); button.className = 'download'; button.textContent = '下载'
        button.addEventListener('click', () => sendResource(selected, button))
        const pushButton = document.createElement('button'); pushButton.className = 'download push-tv'; pushButton.textContent = '推电视'
        pushButton.title = '推送到电视播放'
        pushButton.addEventListener('click', () => pushToTv(selected, pushButton))
        actions.append(button, pushButton)
        row.append(meta, actions); list.append(row)
      })
      updateVideoButtons()
    }
    let positionFrame = 0
    const scheduleVideoButtons = () => {
      if (positionFrame) return
      positionFrame = requestAnimationFrame(() => { positionFrame = 0; updateVideoButtons() })
    }
    window.addEventListener('scroll', scheduleVideoButtons, { capture: true, passive: true })
    window.addEventListener('resize', scheduleVideoButtons)
    new MutationObserver(scheduleVideoButtons).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['src', 'style', 'class'] })

    const markPlayback = (event: Event) => {
      const video = event.target instanceof HTMLVideoElement ? event.target : null
      if (!video) return
      const rect = video.getBoundingClientRect()
      if (rect.width < 180 || rect.height < 100 || rect.bottom < 0 || rect.top > innerHeight || rect.right < 0 || rect.left > innerWidth) return
      const visibleVideos = [...document.querySelectorAll<HTMLVideoElement>('video')]
        .map(item => ({ item, rect: item.getBoundingClientRect() }))
        .filter(item => item.rect.width >= 180 && item.rect.height >= 100 && item.rect.bottom >= 0 && item.rect.top <= innerHeight && item.rect.right >= 0 && item.rect.left <= innerWidth)
        .sort((left, right) => right.rect.width * right.rect.height - left.rect.width * left.rect.height)
      if (visibleVideos[0]?.item !== video) return
      const sourceUrls = [video.currentSrc, video.src, ...[...video.querySelectorAll<HTMLSourceElement>('source[src]')].map(source => source.src)].filter(Boolean)
      const changedSource = sourceUrls.join('\n') !== (activePlayback?.sourceUrls || []).join('\n')
      if (!activePlayback || changedSource) activePlayback = { sourceUrls, startedAt: Date.now() }
      render()
    }
    document.addEventListener('play', markPlayback, true)
    document.addEventListener('playing', markPlayback, true)

    const add = (url: string, mimeType = '') => {
      const kind = classifyResource(url, mimeType); if (!kind) return
      let filename = ''
      try { filename = decodeURIComponent(new URL(url).pathname.split('/').pop() || '') } catch {}
      const resource = { id: resourceId(url), url, kind, mimeType, pageUrl: location.href, title: pageMediaTitle() || filename, filename, seenAt: Date.now() }
      addResource(resource); render(); void runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
    }
    window.addEventListener('__hls_downloader_resource__', ((event: CustomEvent) => add(event.detail?.url, event.detail?.mimeType)) as EventListener)
    document.querySelectorAll<HTMLMediaElement>('video[src],audio[src],source[src]').forEach(media => add(media.currentSrc || media.src))
    new PerformanceObserver(list => list.getEntries().forEach(entry => add(entry.name))).observe({ type: 'resource', buffered: true })
    browser.runtime.onMessage.addListener(message => {
      if (message?.type === 'captured-resource' && message.resource?.url) {
        const pageTitle = pageMediaTitle()
        const resource = {
          ...message.resource,
          title: !message.resource.title || isGenericMediaName(message.resource.title) ? pageTitle || message.resource.title : message.resource.title,
          id: resourceId(message.resource.url),
          seenAt: Date.now(),
        } as MediaResource
        addResource(resource); render()
        void runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
        return
      }
      if (message?.type === 'collect-selection') {
        const selection = window.getSelection(); if (!selection?.rangeCount) return
        const root = selection.getRangeAt(0).cloneContents()
        root.querySelectorAll<HTMLAnchorElement>('a[href]').forEach(anchor => add(anchor.href))
      }
    })
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
      resources.clear(); render(); loadPageResources(location.href)
      document.querySelectorAll<HTMLMediaElement>('video[src],audio[src],source[src]').forEach(media => add(media.currentSrc || media.src))
    }
    loadPageResources(location.href)
    window.addEventListener('popstate', syncPage)
    window.addEventListener('hashchange', syncPage)
    window.setInterval(syncPage, 800)
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
