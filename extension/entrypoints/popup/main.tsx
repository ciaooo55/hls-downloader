import React, { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { browser } from 'wxt/browser'
import { Download, ExternalLink, Power, ShieldCheck } from 'lucide-react'
import type { MediaResource } from '../../lib/resources'
import './style.css'

function App() {
  const [resources, setResources] = useState<MediaResource[]>([])
  const [online, setOnline] = useState(false)
  const [enabled, setEnabled] = useState(true)
  const [host, setHost] = useState('')
  const [authorized, setAuthorized] = useState<string[]>([])
  useEffect(() => { void (async () => {
    const [tab] = await browser.tabs.query({ active: true, currentWindow: true })
    const pageUrl = tab?.url || ''
    try { setHost(new URL(pageUrl).host) } catch {}
    setResources(await browser.runtime.sendMessage({ type: 'list', pageUrl }) || [])
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
  return <main>
    <header><div><h1>HLS Downloader</h1><span className={online ? 'online' : ''}>{online ? '桌面端已连接' : '桌面端离线'}</span></div><button title="打开桌面端" onClick={() => browser.runtime.sendNativeMessage('com.ciaooo55.hls_downloader', { op: 'activate' })}><ExternalLink size={17}/></button></header>
    <div className="controls"><button onClick={async () => { const value = !enabled; setEnabled(value); await browser.storage.local.set({ enabled: value }) }}><Power size={16}/>{enabled ? '自动接管已开启' : '自动接管已关闭'}</button><button onClick={toggleCookie}><ShieldCheck size={16}/>{authorized.includes(host) ? '已授权本站 Cookie' : '授权本站 Cookie'}</button></div>
    <section><div className="section-title">当前页面资源 <b>{resources.length}</b></div>{resources.length ? resources.map(item => <article key={item.id}><div><strong>{item.title || item.url.split('/').pop()}</strong><span>{item.kind.toUpperCase()} · {new URL(item.url).host}</span></div><button title="发送到下载器" onClick={() => browser.runtime.sendMessage({ type: 'offer', resource: item })}><Download size={16}/></button></article>) : <p className="empty">播放媒体后，这里会显示可下载资源。</p>}</section>
    <footer>Alt 绕过接管 · Ctrl 强制接管</footer>
  </main>
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><App/></React.StrictMode>)
