import { useCallback, useEffect, useRef, useState } from 'react'
import { LoaderCircle, RefreshCw, X } from 'lucide-react'
import { fetchBrowserHandoff, fetchBrowserHandoffs, fetchSettings, resolveBrowserHandoff } from './api'
import { closeDesktopWindow } from './desktop'
import { loadHandoffPresentation, pendingHandoffCount } from './handoffWindowLoad'
import { resolveTheme } from './theme'
import type { Settings } from './types'
import BrowserHandoffDialog, { type BrowserHandoff, type BrowserHandoffCancelDecision, type BrowserHandoffDecision } from './components/BrowserHandoffDialog'
import WindowChrome from './components/WindowChrome'
import { isTauriDesktop } from './tauri'

export default function BrowserHandoffWindow({
  handoffId,
  persistent = false,
  initialSettings = {},
  onClosed,
}: {
  handoffId: string
  persistent?: boolean
  initialSettings?: Settings
  onClosed?: (handoffId: string) => void
}) {
  const [item, setItem] = useState<BrowserHandoff | null>(null)
  const [settings, setSettings] = useState<Settings>(initialSettings)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [queueRemaining, setQueueRemaining] = useState(0)
  const resolvedRef = useRef(false)
  const closingRef = useRef(false)
  const onClosedRef = useRef(onClosed)
  onClosedRef.current = onClosed

  useEffect(() => {
    if (!initialSettings.download_dir && !Object.keys(initialSettings.browser_category_dirs || {}).length) return
    setSettings(current => current.download_dir ? current : initialSettings)
  }, [initialSettings])

  const close = useCallback(() => {
    if (closingRef.current) return
    closingRef.current = true
    if (persistent && isTauriDesktop()) {
      void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
        await getCurrentWindow().hide()
        onClosedRef.current?.(handoffId)
      })
    } else {
      void closeDesktopWindow()
    }
  }, [handoffId, persistent])

  const load = useCallback(async () => {
    if (closingRef.current || resolvedRef.current) return
    setError('')
    try {
      const result = await loadHandoffPresentation(
        handoffId,
        {
          fetchHandoff: fetchBrowserHandoff,
          fetchSettings,
          fetchHandoffs: fetchBrowserHandoffs,
        },
        {
          item: handoff => {
            if (closingRef.current || resolvedRef.current) return
            setItem(handoff)
          },
          extras: ({ settings: currentSettings, queueRemaining: remaining }) => {
            if (closingRef.current || resolvedRef.current) return
            setSettings(currentSettings)
            setQueueRemaining(remaining)
          },
        },
      )
      if (closingRef.current || resolvedRef.current) return
      if (result.close) close()
    } catch (reason: any) {
      if (closingRef.current || resolvedRef.current) return
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
        if (persistent) {
          await getCurrentWindow().hide().catch(() => {})
          onClosed?.(handoffId)
        } else {
          await getCurrentWindow().destroy().catch(() => {})
        }
      }),
    ).then(cleanup => { unlisten = cleanup })
    return () => unlisten?.()
  }, [handoffId, onClosed, persistent])

  useEffect(() => {
    document.documentElement.dataset.surface = 'handoff'
    document.documentElement.dataset.theme = resolveTheme(
      localStorage.getItem('hls_theme'),
      matchMedia('(prefers-color-scheme: dark)').matches,
    )
    void load()
    const timer = window.setInterval(() => {
      if (resolvedRef.current || closingRef.current || document.hidden) return
      void Promise.all([fetchBrowserHandoff(handoffId), fetchBrowserHandoffs()])
        .then(([handoff, pendingHandoffs]) => {
          if (resolvedRef.current || closingRef.current) return
          if (handoff.status && handoff.status !== 'pending') close()
          else {
            setItem(handoff)
            setQueueRemaining(pendingHandoffCount(handoffId, pendingHandoffs))
          }
        })
        .catch(() => {})
    }, 2000)
    return () => window.clearInterval(timer)
  }, [handoffId, load, close])

  const resolve = async (action: 'accept' | 'cancel', decision?: BrowserHandoffDecision | BrowserHandoffCancelDecision) => {
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
    return <main className="handoff-window-root has-window-chrome">
      <WindowChrome resizable />
      {error && <div className="handoff-window-error">{error}</div>}
      <BrowserHandoffDialog item={item} busy={busy} settings={settings} onResolve={resolve} standalone queueRemaining={queueRemaining} />
    </main>
  }

  return <main className="handoff-window-root has-window-chrome handoff-window-loading">
    <WindowChrome resizable />
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
