import React, { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { browser } from 'wxt/browser'
import { Download, ExternalLink, Power, ShieldCheck, X } from 'lucide-react'
import type { MediaResource } from '../../lib/resources'
import { resourceQuality } from '../../lib/hlsManifest'
import './style.css'

function App() {
  const [resources, setResources] = useState<MediaResource[]>([])
  const [online, setOnline] = useState(false)
  const [enabled, setEnabled] = useState(true)
  const [host, setHost] = useState('')
  const [authorized, setAuthorized] = useState<string[]>([])
  const [sending, setSending] = useState<Record<string, string>>({})
  const [error, setError] = useState('')
  useEffect(() => { void (async () => {
    const [tab] = await browser.tabs.query({ active: true, currentWindow: true })
    const pageUrl = tab?.url || ''
    try { setHost(new URL(pageUrl).host) } catch {}
    setResources(await browser.runtime.sendMessage({ type: 'list', pageUrl, tabId: tab?.id }) || [])
    setOnline(Boolean((await browser.runtime.sendMessage({ type: 'ping' }))?.ok))
    const stored = await browser.storage.local.get(['enabled', 'authorizedCookieHosts'])
    setEnabled(stored.enabled !== false)
    setAuthorized(Array.isArray(stored.authorizedCookieHosts) ? stored.authorizedCookieHosts : [])
  })() }, [])
  const toggleCookie = async () => {
    if (!host) return
    const next = authorized.includes(host) ? authorized.filter(value => value !== host) : [...authorized, host]
    setAuthorized(next); await browser.storage.local.set({ authorizedCookieHosts: next })
  }
  const send = async (item: MediaResource) => {
    setError(''); setSending(current => ({ ...current, [item.id]: '发送中' }))
    try {
      const response = await browser.runtime.sendMessage({ type: 'download', resource: item })
      if (!response?.ok || !response?.task?.id) throw new Error(response?.error || '桌面端没有返回任务编号')
      setSending(current => ({ ...current, [item.id]: '已加入' }))
    } catch (reason) {
      setSending(current => ({ ...current, [item.id]: '重试' }))
      setError(reason instanceof Error ? reason.message : '发送到桌面端失败')
    }
  }
  return <main>
    <header><div className="brand"><img src="/icon.png" alt=""/><div><h1>HLS Downloader</h1><span className={online ? 'online' : ''}>{online ? '桌面端已连接' : '桌面端离线'}</span></div></div><div className="header-actions"><button title="打开桌面端" onClick={() => browser.runtime.sendMessage({ type: 'activate' })}><ExternalLink size={17}/></button><button className="close-button" title="关闭" onClick={() => window.close()}><X size={18}/></button></div></header>
    <div className="controls"><button onClick={async () => { const value = !enabled; setEnabled(value); await browser.storage.local.set({ enabled: value }) }}><Power size={16}/>{enabled ? '自动接管已开启' : '自动接管已关闭'}</button><button onClick={toggleCookie}><ShieldCheck size={16}/>{authorized.includes(host) ? '已授权本站 Cookie' : '授权本站 Cookie'}</button></div>
    {error && <div className="send-error">{error}</div>}
    <section><div className="section-title">当前页面资源 <b>{resources.length}</b></div>{resources.length ? resources.map(item => <ResourceRow key={item.id} item={item} status={sending[item.id]} onSend={() => send(item)} />) : <p className="empty">播放媒体后，这里会显示可下载资源。</p>}</section>
    <footer>Alt 绕过接管 · Ctrl 强制接管</footer>
  </main>
}

function ResourceRow({ item, status, onSend }: { item: MediaResource; status?: string; onSend: () => void }) {
  let host = item.url
  try { host = new URL(item.url).host } catch {}
  const size = item.size && item.size > 0 ? formatSize(item.size) : '大小未知'
  const quality = item.quality || resourceQuality(item.url, item.height)
  const resolution = item.width && item.height ? `${item.width}×${item.height}` : ''
  const bandwidth = item.bandwidth ? `${(item.bandwidth / 1_000_000).toFixed(1)} Mbps` : ''
  return <article><div><strong title={item.filename || item.title || item.url}>{item.title || item.filename || item.url.split('/').pop() || item.url}</strong><span>{[item.kind.toUpperCase(), quality, resolution, bandwidth, size, item.statusCode ? `HTTP ${item.statusCode}` : '', item.method].filter(Boolean).join(' · ')}</span><small title={[item.mimeType, host].filter(Boolean).join(' · ')}>{[item.mimeType, host].filter(Boolean).join(' · ')}</small><small className="resource-url" title={item.url}>{item.url}</small></div><button disabled={status === '发送中' || status === '已加入'} title="发送到下载器" onClick={onSend}>{status ? <em>{status}</em> : <Download size={16}/>}</button></article>
}

function formatSize(size: number) {
  const units = ['B', 'KB', 'MB', 'GB']; let value = size; let index = 0
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index += 1 }
  return `${value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><App/></React.StrictMode>)
