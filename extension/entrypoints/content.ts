import { browser } from 'wxt/browser'
import { classifyResource, isGenericMediaName, resourceId, type MediaResource } from '../lib/resources'
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
          .item{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:10px;padding:12px;border-bottom:1px solid #e7f4f8}.item:hover{background:#f7fcff}.meta{min-width:0}.name{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;font:600 13px/1.35 system-ui;overflow-wrap:anywhere}.url{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;color:#54717f;font:11px/1.35 system-ui;margin-top:4px;overflow-wrap:anywhere}.item:hover .name,.item:hover .url{-webkit-line-clamp:unset}.kind{color:#25627b;font:12px system-ui;margin-top:4px}.download{align-self:center;min-width:58px;height:32px;border:0;border-radius:6px;background:#0ea5e9;color:white;padding:6px 10px;cursor:pointer;font-weight:600}.download:hover{background:#0284c7}.download[disabled]{cursor:default;opacity:.65}.result{padding:8px 12px;background:#ecfdf5;color:#047857;font:12px/1.4 system-ui}.result.error{background:#fff1f2;color:#be123c}
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
        root.append(style, panelWrap)
        container.append(root)
        const wrap = root.querySelector('.wrap')!
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
      const position = value.panelPosition
      pinned = value.panelPinned === true
      pinButton?.classList.toggle('active', pinned)
      if (pinButton) pinButton.textContent = pinned ? '已固定' : '固定'
      if (pinned) setOpen(true)
      if (wrap && position && Number.isFinite(position.x) && Number.isFinite(position.y)) {
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

    const render = () => {
      const list = ui.shadow.querySelector('.list')
      if (!list) return
      const entries = [...resources.values()]
      list.replaceChildren()
      if (!entries.length) {
        const empty = document.createElement('div')
        empty.className = 'empty'
        empty.textContent = '播放视频后会显示资源'
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
        const kind = document.createElement('div'); kind.className = 'kind'; kind.textContent = [resource.kind.toUpperCase(), quality, resource.width && resource.height ? `${resource.width}×${resource.height}` : '', bandwidth, duration, resource.size ? formatSize(resource.size) : '大小未知', host].filter(Boolean).join(' · ')
        const url = document.createElement('div'); url.className = 'url'; url.title = resource.url; url.textContent = resource.url
        const button = document.createElement('button'); button.className = 'download'; button.textContent = '下载'
        button.addEventListener('click', () => {
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
            button.removeAttribute('disabled')
            button.textContent = '重试'
          }).catch(reason => {
            button.removeAttribute('disabled'); button.textContent = '重试'
            if (result) { result.hidden = false; result.classList.add('error'); result.textContent = reason?.message || String(reason) || '发送失败' }
          })
        })
        meta.append(name, kind, url); row.append(meta, button); list.append(row)
      })
    }
    const add = (url: string, mimeType = '') => {
      const kind = classifyResource(url, mimeType); if (!kind) return
      let filename = ''
      try { filename = decodeURIComponent(new URL(url).pathname.split('/').pop() || '') } catch {}
      const resource = { id: resourceId(url), url, kind, mimeType, pageUrl: location.href, title: pageMediaTitle() || filename, filename, seenAt: Date.now() }
      resources.set(url, resource); render(); void runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
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
        resources.set(resource.url, resource); render()
        void runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
        return
      }
      if (message?.type === 'collect-selection') {
        const selection = window.getSelection(); if (!selection?.rangeCount) return
        const root = selection.getRangeAt(0).cloneContents()
        root.querySelectorAll<HTMLAnchorElement>('a[href]').forEach(anchor => add(anchor.href))
      }
    })
    void runtimeMessage({ type: 'list', pageUrl: location.href }).then((stored: MediaResource[]) => {
      if (!Array.isArray(stored)) return
      stored.forEach(resource => {
        if (resource?.url) resources.set(resource.url, {
          ...resource,
          title: !resource.title || isGenericMediaName(resource.title) ? pageMediaTitle() || resource.title : resource.title,
        })
      })
      render()
    }).catch(() => undefined)
  },
})

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
