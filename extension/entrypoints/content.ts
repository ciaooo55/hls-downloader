import { browser } from 'wxt/browser'
import { classifyResource, resourceId, type MediaResource } from '../lib/resources'
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
    const ui = await createShadowRootUi(ctx, {
      name: 'hls-downloader-media-panel', position: 'inline', anchor: 'body',
      onMount(container) {
        const root = document.createElement('div')
        const iconUrl = browser.runtime.getURL('/icon.png')
        root.innerHTML = `<style>
          :host{all:initial}*{box-sizing:border-box}button{font:13px system-ui,sans-serif;letter-spacing:0}
          .wrap{position:fixed;right:14px;top:35%;z-index:2147483647;color:#102a3a;filter:drop-shadow(0 5px 8px #07598529)}
          .toggle{display:grid;place-items:center;width:46px;height:46px;padding:3px;border:2px solid #38bdf8;border-radius:11px;background:#f0fbff;cursor:pointer}.toggle img{width:38px;height:38px}
          .panel{display:none;width:min(420px,calc(100vw - 20px));max-height:70vh;background:#fff;border:1px solid #bae6fd;border-radius:9px;overflow:hidden}.open .panel{display:block}.open .toggle{display:none}
          header{display:flex;align-items:center;justify-content:space-between;padding:9px 10px 9px 12px;border-bottom:1px solid #dff5ff;background:#f0fbff;font:600 14px system-ui}.title{display:flex;align-items:center;gap:8px}.title img{width:24px;height:24px}.head-actions{display:flex;align-items:center;gap:5px}
          .pin,.close{height:30px;border:0;border-radius:5px;background:#e0f2fe;color:#075985;cursor:pointer}.pin{padding:0 9px;font:12px system-ui}.pin.active{background:#d1fae5;color:#047857}.close{display:grid;place-items:center;width:30px;font:700 20px/1 system-ui}.list{overflow:auto;max-height:58vh}.empty{padding:20px;color:#526b79;font:13px system-ui}
          .item{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:10px;padding:12px;border-bottom:1px solid #e7f4f8}.item:hover{background:#f7fcff}.meta{min-width:0}.name{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;font:600 13px/1.35 system-ui;overflow-wrap:anywhere}.url{display:-webkit-box;overflow:hidden;-webkit-line-clamp:2;-webkit-box-orient:vertical;color:#54717f;font:11px/1.35 system-ui;margin-top:4px;overflow-wrap:anywhere}.item:hover .name,.item:hover .url{-webkit-line-clamp:unset}.kind{color:#25627b;font:12px system-ui;margin-top:4px}.download{align-self:center;min-width:58px;height:32px;border:0;border-radius:6px;background:#0ea5e9;color:white;padding:6px 10px;cursor:pointer;font-weight:600}.download:hover{background:#0284c7}.download[disabled]{cursor:default;opacity:.65}.result{padding:8px 12px;background:#ecfdf5;color:#047857;font:12px/1.4 system-ui}.result.error{background:#fff1f2;color:#be123c}
          button:focus-visible{outline:2px solid #0369a1;outline-offset:2px}@media(prefers-reduced-motion:reduce){*{transition:none!important}}
        </style><div class="wrap"><button class="toggle" title="媒体嗅探：悬停展开" aria-label="展开媒体嗅探"><img src="${iconUrl}" alt=""></button><div class="panel"><header><span class="title"><img src="${iconUrl}" alt="">检测到的媒体</span><div class="head-actions"><button class="pin" title="固定展开">固定</button><button class="close" title="折叠" aria-label="折叠">×</button></div></header><div class="result" hidden></div><div class="list"><div class="empty">播放视频后会显示资源</div></div></div></div>`
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
        wrap.style.left = `${Math.max(0, Math.min(innerWidth - 42, startLeft + next.clientX - startX))}px`
        wrap.style.top = `${Math.max(0, Math.min(innerHeight - 42, startTop + next.clientY - startY))}px`
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
      list.innerHTML = entries.length ? '' : '<div class="empty">播放视频后会显示资源</div>'
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
          void runtimeMessage({ type: 'offer', resource }).then(response => {
            if (!response?.ok || !response?.handoff?.id) throw new Error(response?.error || '桌面端未接受请求')
            button.textContent = '待确认'
            if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `请在桌面下载器确认：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
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
      const resource = { id: resourceId(url), url, kind, mimeType, pageUrl: location.href, title: filename || document.title, filename, seenAt: Date.now() }
      resources.set(url, resource); render(); void runtimeMessage({ type: 'resource', resource }).catch(() => undefined)
    }
    window.addEventListener('__hls_downloader_resource__', ((event: CustomEvent) => add(event.detail?.url, event.detail?.mimeType)) as EventListener)
    document.querySelectorAll<HTMLMediaElement>('video[src],audio[src],source[src]').forEach(media => add(media.currentSrc || media.src))
    new PerformanceObserver(list => list.getEntries().forEach(entry => add(entry.name))).observe({ type: 'resource', buffered: true })
    browser.runtime.onMessage.addListener(message => {
      if (message?.type === 'captured-resource' && message.resource?.url) {
        const resource = { ...message.resource, id: resourceId(message.resource.url), seenAt: Date.now() } as MediaResource
        resources.set(resource.url, resource); render()
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
        if (resource?.url) resources.set(resource.url, resource)
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
