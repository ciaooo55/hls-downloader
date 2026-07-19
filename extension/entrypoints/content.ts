import { browser } from 'wxt/browser'
import { classifyResource, resourceId, type MediaResource } from '../lib/resources'

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
        root.innerHTML = `<style>
          :host{all:initial}*{box-sizing:border-box}button{font:13px system-ui,sans-serif;letter-spacing:0}
          .wrap{position:fixed;right:14px;top:35%;z-index:2147483647;color:#17202a;filter:drop-shadow(0 4px 12px #0003)}
          .toggle{width:42px;height:42px;border:0;border-radius:7px;background:#1267a8;color:white;cursor:pointer;font-weight:700}
          .panel{display:none;width:300px;max-height:52vh;background:#fff;border:1px solid #ccd3da;border-radius:7px;overflow:hidden}.open .panel{display:block}.open .toggle{display:none}
          header{display:flex;align-items:center;justify-content:space-between;padding:10px 12px;border-bottom:1px solid #e4e8ec;background:#f7f9fa;font:600 14px system-ui}
          .close{border:0;background:transparent;cursor:pointer;font-size:20px}.list{overflow:auto;max-height:43vh}.empty{padding:18px;color:#68727c;font:13px system-ui}
          .item{display:grid;grid-template-columns:1fr auto;gap:8px;padding:10px 12px;border-bottom:1px solid #edf0f2}.meta{min-width:0}.name{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font:600 13px system-ui}.kind{color:#68727c;font:12px system-ui;margin-top:3px}.download{border:0;border-radius:5px;background:#176b48;color:white;padding:6px 9px;cursor:pointer}
        </style><div class="wrap"><button class="toggle" title="媒体嗅探">DL</button><div class="panel"><header><span>检测到的媒体</span><button class="close" title="折叠">×</button></header><div class="list"><div class="empty">播放视频后会显示资源</div></div></div></div>`
        container.append(root)
        const wrap = root.querySelector('.wrap')!
        root.querySelector('.toggle')!.addEventListener('click', () => wrap.classList.add('open'))
        root.querySelector('.close')!.addEventListener('click', () => wrap.classList.remove('open'))
        return root
      },
    })
    ui.mount()
    window.addEventListener('click', event => {
      if (event.altKey || event.ctrlKey) void browser.runtime.sendMessage({ type: 'modifier', altKey: event.altKey, ctrlKey: event.ctrlKey })
    }, true)

    const wrap = ui.shadow.querySelector<HTMLElement>('.wrap')
    const dragHandles = ui.shadow.querySelectorAll<HTMLElement>('.toggle, header')
    let dragged = false
    void browser.storage.local.get('panelPosition').then(value => {
      const position = value.panelPosition
      if (wrap && position && Number.isFinite(position.x) && Number.isFinite(position.y)) {
        wrap.style.left = `${Math.max(0, position.x)}px`; wrap.style.top = `${Math.max(0, position.y)}px`; wrap.style.right = 'auto'
      }
    })
    dragHandles.forEach(handle => handle.addEventListener('pointerdown', event => {
      if (!wrap || (event.target as HTMLElement).closest('.close')) return
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
      }
      window.addEventListener('pointermove', move); window.addEventListener('pointerup', finish, { once: true })
    }))
    ui.shadow.querySelector('.toggle')?.addEventListener('click', event => {
      if (dragged) event.stopImmediatePropagation()
    }, true)

    const render = () => {
      const list = ui.shadow.querySelector('.list')
      if (!list) return
      const entries = [...resources.values()]
      list.innerHTML = entries.length ? '' : '<div class="empty">播放视频后会显示资源</div>'
      entries.forEach(resource => {
        const row = document.createElement('div'); row.className = 'item'
        const meta = document.createElement('div'); meta.className = 'meta'
        const name = document.createElement('div'); name.className = 'name'; name.textContent = resource.title || resource.url.split('/').pop() || resource.url
        const kind = document.createElement('div'); kind.className = 'kind'; kind.textContent = resource.kind.toUpperCase()
        const button = document.createElement('button'); button.className = 'download'; button.textContent = '下载'
        button.addEventListener('click', () => void browser.runtime.sendMessage({ type: 'offer', resource }))
        meta.append(name, kind); row.append(meta, button); list.append(row)
      })
    }
    const add = (url: string, mimeType = '') => {
      const kind = classifyResource(url, mimeType); if (!kind) return
      const resource = { id: resourceId(url), url, kind, mimeType, pageUrl: location.href, title: document.title, seenAt: Date.now() }
      resources.set(url, resource); render(); void browser.runtime.sendMessage({ type: 'resource', resource })
    }
    window.addEventListener('__hls_downloader_resource__', ((event: CustomEvent) => add(event.detail?.url, event.detail?.mimeType)) as EventListener)
    document.querySelectorAll<HTMLMediaElement>('video[src],audio[src],source[src]').forEach(media => add(media.currentSrc || media.src))
    new PerformanceObserver(list => list.getEntries().forEach(entry => add(entry.name))).observe({ type: 'resource', buffered: true })
    browser.runtime.onMessage.addListener(message => {
      if (message?.type !== 'collect-selection') return
      const selection = window.getSelection(); if (!selection?.rangeCount) return
      const root = selection.getRangeAt(0).cloneContents()
      root.querySelectorAll<HTMLAnchorElement>('a[href]').forEach(anchor => add(anchor.href))
    })
  },
})
