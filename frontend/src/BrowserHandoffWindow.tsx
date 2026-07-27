import { useCallback, useEffect, useRef, useState } from 'react'
import { LoaderCircle, RefreshCw, X } from 'lucide-react'
import { fetchBrowserHandoff, fetchSettings, resolveBrowserHandoff } from './api'
import { closeDesktopWindow, resizeDesktopWindow } from './desktop'
import { resolveTheme } from './theme'
import type { Settings } from './types'
import BrowserHandoffDialog, { type BrowserHandoff, type BrowserHandoffDecision } from './components/BrowserHandoffDialog'
import WindowChrome from './components/WindowChrome'
import { isTauriDesktop } from './tauri'

export default function BrowserHandoffWindow({ handoffId }: { handoffId: string }) {
  const [item, setItem] = useState<BrowserHandoff | null>(null)
  const [settings, setSettings] = useState<Settings>({})
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [closing, setClosing] = useState(false)
  const resolvedRef = useRef(false)
  const surfaceRef = useRef<HTMLElement | null>(null)

  const close = useCallback(() => {
    if (closing) return
    setClosing(true)
    void closeDesktopWindow()
  }, [closing])

  const load = useCallback(async () => {
    setError('')
    try {
      const [handoff, currentSettings] = await Promise.all([
        fetchBrowserHandoff(handoffId),
        fetchSettings(),
      ])
      if (handoff.status && handoff.status !== 'pending') {
        close()
        return
      }
      setItem(handoff)
      setSettings(currentSettings)
    } catch (reason: any) {
      setError(reason?.message || '无法读取浏览器下载请求')
    }
  }, [close, handoffId])

  useEffect(() => {
    if (!isTauriDesktop()) return
    let unlisten: (() => void) | undefined
    void import('@tauri-apps/api/window').then(({ getCurrentWindow }) =>
      getCurrentWindow().onCloseRequested(async event => {
        if (resolvedRef.current) return
        event.preventDefault()
        resolvedRef.current = true
        await resolveBrowserHandoff(handoffId, 'cancel').catch(() => {})
        await getCurrentWindow().destroy().catch(() => {})
      }),
    ).then(cleanup => { unlisten = cleanup })
    return () => unlisten?.()
  }, [handoffId])

  useEffect(() => {
    document.documentElement.dataset.surface = 'handoff'
    document.documentElement.dataset.theme = resolveTheme(
      localStorage.getItem('hls_theme'),
      matchMedia('(prefers-color-scheme: dark)').matches,
    )
    void load()
    const timer = window.setInterval(() => {
      if (resolvedRef.current || document.hidden) return
      void fetchBrowserHandoff(handoffId)
        .then(handoff => {
          if (handoff.status && handoff.status !== 'pending') close()
          else setItem(handoff)
        })
        .catch(() => {})
    }, 2000)
    return () => window.clearInterval(timer)
  }, [close, handoffId, load])

  useEffect(() => {
    const surface = surfaceRef.current
    if (!surface || typeof ResizeObserver === 'undefined') return
    let last = ''
    let frame = 0
    const resize = () => {
      frame = 0
      const rect = surface.getBoundingClientRect()
      const width = Math.ceil(Math.max(360, Math.min(420, rect.width || 400)))
      const height = Math.ceil(Math.max(300, Math.min(680, surface.scrollHeight + 2)))
      const key = `${width}x${height}`
      if (key === last) return
      last = key
      void resizeDesktopWindow(width, height)
    }
    const observer = new ResizeObserver(() => {
      if (!frame) frame = requestAnimationFrame(resize)
    })
    observer.observe(surface)
    resize()
    return () => { observer.disconnect(); if (frame) cancelAnimationFrame(frame) }
  }, [item, error])

  const resolve = async (action: 'accept' | 'cancel', decision?: BrowserHandoffDecision) => {
    if (busy || resolvedRef.current) return
    setBusy(true)
    setError('')
    try {
      await resolveBrowserHandoff(handoffId, action, decision)
      resolvedRef.current = true
      close()
    } catch (reason: any) {
      setError(reason?.message || '浏览器接管操作失败')
      setBusy(false)
    }
  }

  if (item) {
    return <main ref={surfaceRef} className="handoff-window-root has-window-chrome">
      <WindowChrome />
      {error && <div className="handoff-window-error">{error}</div>}
      <BrowserHandoffDialog item={item} busy={busy} settings={settings} onResolve={resolve} standalone />
    </main>
  }

  return <main ref={surfaceRef} className="handoff-window-root has-window-chrome handoff-window-loading">
    <WindowChrome />
    <section>
      {error ? <>
        <X size={28} />
        <strong>下载窗口加载失败</strong>
        <p>{error}</p>
        <div>
          <button className="secondary-button" onClick={close}>关闭</button>
          <button className="primary-button" onClick={() => void load()}><RefreshCw size={15} />重试</button>
        </div>
      </> : <>
        <LoaderCircle className="spin" size={28} />
        <strong>正在准备下载窗口</strong>
      </>}
    </section>
  </main>
}
