import { useCallback, useEffect, useState } from 'react'
import { launchFile, openTaskInExplorer } from './api'
import OverlayChrome from './components/OverlayChrome'
import DownloadCompletePanel from './components/DownloadCompletePanel'
import {
  dismissCompleteItem,
  enqueueCompleteItem,
  type DownloadCompleteItem,
} from './downloadOverlay'
import { resolveTheme } from './theme'

function applyOverlayTheme() {
  document.documentElement.dataset.surface = 'overlay'
  document.documentElement.dataset.theme = resolveTheme(
    localStorage.getItem('hls_theme'),
    matchMedia('(prefers-color-scheme: dark)').matches,
  )
}

export default function DownloadCompleteHost() {
  const [queue, setQueue] = useState<DownloadCompleteItem[]>([])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const notifyEmpty = useCallback((next: DownloadCompleteItem[]) => {
    if (next.length) return
    void import('@tauri-apps/api/event').then(({ emitTo }) => emitTo('main', 'download-complete-empty', {}))
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      await getCurrentWindow().hide().catch(() => {})
    })
  }, [])

  const closeCurrent = useCallback(() => {
    setQueue(current => {
      const next = dismissCompleteItem(current, current[0]?.id || '')
      notifyEmpty(next)
      return next
    })
    setError('')
    setBusy(false)
  }, [notifyEmpty])

  useEffect(() => {
    applyOverlayTheme()
    let unlisten: (() => void) | undefined
    let unlistenClose: (() => void) | undefined
    void import('@tauri-apps/api/event').then(async ({ emitTo, listen }) => {
      unlisten = await listen<{ item?: DownloadCompleteItem }>('download-complete-enqueue', event => {
        const item = event.payload?.item
        if (!item?.id) return
        setQueue(current => enqueueCompleteItem(current, item))
        setError('')
        void emitTo('main', 'download-complete-ready', {})
      })
      await emitTo('main', 'download-complete-host-ready', {})
    })
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      unlistenClose = await getCurrentWindow().onCloseRequested(async event => {
        event.preventDefault()
        closeCurrent()
      })
    })
    return () => {
      unlisten?.()
      unlistenClose?.()
    }
  }, [closeCurrent])

  const item = queue[0]
  if (!item) {
    return (
      <main className="overlay-window-root download-complete-host" aria-hidden="true">
        <OverlayChrome title="下载完成" onClose={closeCurrent} />
      </main>
    )
  }

  return (
    <main className="overlay-window-root download-complete-host">
      <OverlayChrome title="下载完成" onClose={closeCurrent} />
      <DownloadCompletePanel
        item={item}
        remaining={Math.max(0, queue.length - 1)}
        busy={busy}
        error={error}
        onClose={closeCurrent}
        onOpenFolder={() => {
          void (async () => {
            setBusy(true)
            setError('')
            try {
              await openTaskInExplorer(item.id)
              closeCurrent()
            } catch (reason: any) {
              setError(reason?.message || '无法打开目录')
            } finally {
              setBusy(false)
            }
          })()
        }}
        onOpenFile={confirmed => {
          void (async () => {
            setBusy(true)
            setError('')
            try {
              await launchFile(item.id, confirmed)
              closeCurrent()
            } catch (reason: any) {
              setError(reason?.message || '无法打开文件')
            } finally {
              setBusy(false)
            }
          })()
        }}
      />
    </main>
  )
}
