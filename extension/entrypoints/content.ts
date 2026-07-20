import { browser } from 'wxt/browser'
import { classifyResource, isDirectDownloadLink, resourceId, type MediaResource } from '../lib/resources'

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
          .panel{display:none;width:min(340px,calc(100vw - 20px));max-height:60vh;background:#fff;border:1px solid #ccd3da;border-radius:7px;overflow:hidden}.open .panel{display:block}.open .toggle{display:none}
          header{display:flex;align-items:center;justify-content:space-between;padding:10px 12px;border-bottom:1px solid #e4e8ec;background:#f7f9fa;font:600 14px system-ui}
          .close{display:grid;place-items:center;width:30px;height:30px;border:0;border-radius:5px;background:#e7ebee;color:#25313a;cursor:pointer;font:700 20px/1 system-ui}.list{overflow:auto;max-height:49vh}.empty{padding:18px;color:#68727c;font:13px system-ui}
          .item{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:9px;padding:10px 12px;border-bottom:1px solid #edf0f2}.meta{min-width:0}.name,.url{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.name{font:600 13px system-ui}.url{color:#84909a;font:11px system-ui;margin-top:3px}.kind{color:#5f6d77;font:12px system-ui;margin-top:3px}.download{min-width:58px;height:30px;border:0;border-radius:5px;background:#176b48;color:white;padding:6px 9px;cursor:pointer}.download[disabled]{cursor:default;opacity:.7}.result{padding:7px 12px;background:#eef6f1;color:#176b48;font:12px system-ui}.result.error{background:#fff0f0;color:#a13737}
        </style><div class="wrap"><button class="toggle" title="媒体嗅探">DL</button><div class="panel"><header><span>检测到的媒体</span><button class="close" title="关闭并折叠" aria-label="关闭并折叠">×</button></header><div class="result" hidden></div><div class="list"><div class="empty">播放视频后会显示资源</div></div></div></div>`
        container.append(root)
        const wrap = root.querySelector('.wrap')!
        root.querySelector('.toggle')!.addEventListener('click', () => {
          wrap.classList.add('open')
          const rect = wrap.getBoundingClientRect()
          wrap.style.left = `${Math.max(10, Math.min(rect.left, innerWidth - rect.width - 10))}px`
          wrap.style.top = `${Math.max(10, Math.min(rect.top, innerHeight - rect.height - 10))}px`
          wrap.style.right = 'auto'
        })
        root.querySelector('.close')!.addEventListener('click', () => wrap.classList.remove('open'))
        return root
      },
    })
    ui.mount()
    window.addEventListener('click', event => {
      if (!event.isTrusted || event.button !== 0) return
      const path = event.composedPath()
      const anchor = path.find(value => value instanceof HTMLAnchorElement) as HTMLAnchorElement | undefined
      const control = path.find(value => value instanceof HTMLElement
        && value.matches('button, input[type="button"], input[type="submit"], [role="button"]'))
      if (!anchor && !control) return
      if (anchor && !event.altKey && (event.ctrlKey || isDirectDownloadLink(anchor.href, anchor.hasAttribute('download')))) {
        event.preventDefault()
        event.stopImmediatePropagation()
        const filename = anchor.download || anchor.href.split(/[?#]/, 1)[0].split('/').pop() || ''
        const resource = {
          id: resourceId(anchor.href), url: anchor.href, kind: classifyResource(anchor.href) || 'file' as const,
          filename, title: anchor.textContent?.trim() || filename, pageUrl: location.href, seenAt: Date.now(),
        }
        const fallbackToBrowser = () => {
          void browser.runtime.sendMessage({
            type: 'browser-download', url: anchor.href, filename: anchor.download || '',
          }).then(response => {
            if (!response?.ok) location.assign(anchor.href)
          }).catch(() => location.assign(anchor.href))
        }
        void browser.runtime.sendMessage({
          type: 'direct-click-download', resource,
          altBypass: false, ctrlForce: event.ctrlKey,
        }).then(response => {
          if (response?.ok && response?.task?.id) return
          fallbackToBrowser()
        }).catch(fallbackToBrowser)
        return
      }
      void browser.runtime.sendMessage({
        type: 'click-intent',
        href: anchor?.href || '',
        pageUrl: location.href,
        altBypass: event.altKey,
        ctrlForce: event.ctrlKey,
      })
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
        const name = document.createElement('div'); name.className = 'name'; name.title = resource.filename || resource.title || resource.url; name.textContent = resource.filename || resource.title || resource.url.split('/').pop() || resource.url
        const kind = document.createElement('div'); kind.className = 'kind'; kind.textContent = [resource.kind.toUpperCase(), resource.mimeType, resource.size ? formatSize(resource.size) : '大小未知'].filter(Boolean).join(' · ')
        const url = document.createElement('div'); url.className = 'url'; url.title = resource.url; url.textContent = resource.url
        const button = document.createElement('button'); button.className = 'download'; button.textContent = '下载'
        button.addEventListener('click', () => {
          const result = ui.shadow.querySelector<HTMLElement>('.result')
          button.setAttribute('disabled', ''); button.textContent = '发送中'
          void browser.runtime.sendMessage({ type: 'download', resource }).then(response => {
            if (!response?.ok || !response?.task?.id) throw new Error(response?.error || '桌面端未接受任务')
            button.textContent = '已加入'
            if (result) { result.hidden = false; result.classList.remove('error'); result.textContent = `已加入桌面下载器：${resource.filename || resource.title || resource.kind.toUpperCase()}` }
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
      const resource = { id: resourceId(url), url, kind, mimeType, pageUrl: location.href, title: document.title, seenAt: Date.now() }
      resources.set(url, resource); render(); void browser.runtime.sendMessage({ type: 'resource', resource })
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
  },
})

function formatSize(size: number): string {
  if (!Number.isFinite(size) || size <= 0) return '大小未知'
  const units = ['B', 'KB', 'MB', 'GB']
  let value = size; let index = 0
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index += 1 }
  return `${value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`
}
