import { useCallback, useEffect, useRef, useState } from 'react'
import { openTaskInExplorer, taskAction } from './api'
import OverlayChrome from './components/OverlayChrome'
import DownloadProgressPanel from './components/DownloadProgressPanel'
import type { DownloadProgressItem } from './downloadOverlay'
import { resolveTheme } from './theme'

function applyOverlayTheme() {
  document.documentElement.dataset.surface = 'overlay'
  document.documentElement.dataset.theme = resolveTheme(
    localStorage.getItem('hls_theme'),
    matchMedia('(prefers-color-scheme: dark)').matches,
  )
}

export default function DownloadProgressHost() {
  const [tasks, setTasks] = useState<DownloadProgressItem[]>([])
  const [busyId, setBusyId] = useState('')
  const tasksRef = useRef<DownloadProgressItem[]>([])
  tasksRef.current = tasks

  const dismiss = useCallback(() => {
    const ids = tasksRef.current.map(task => task.id)
    void import('@tauri-apps/api/event').then(async ({ emitTo }) => {
      await emitTo('main', 'download-progress-dismissed', { ids })
    })
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      await getCurrentWindow().hide().catch(() => {})
    })
  }, [])

  useEffect(() => {
    applyOverlayTheme()
    let unlisten: (() => void) | undefined
    let unlistenClose: (() => void) | undefined
    void import('@tauri-apps/api/event').then(async ({ emitTo, listen }) => {
      unlisten = await listen<{ tasks?: DownloadProgressItem[] }>('download-progress-sync', event => {
        setTasks(Array.isArray(event.payload?.tasks) ? event.payload.tasks : [])
      })
      await emitTo('main', 'download-progress-ready', {})
    })
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      unlistenClose = await getCurrentWindow().onCloseRequested(async event => {
        event.preventDefault()
        dismiss()
      })
    })
    return () => {
      unlisten?.()
      unlistenClose?.()
    }
  }, [dismiss])

  const runAction = async (task: DownloadProgressItem, action: 'pause' | 'resume' | 'cancel') => {
    if (busyId) return
    setBusyId(task.id)
    try {
      await taskAction(task.id, action)
    } finally {
      setBusyId('')
    }
  }

  return (
    <main className="overlay-window-root download-progress-host">
      <OverlayChrome title="正在下载" showMinimize onClose={dismiss} />
      <DownloadProgressPanel
        tasks={tasks}
        busyId={busyId}
        onAction={(task, action) => void runAction(task, action)}
        onOpenFolder={task => { void openTaskInExplorer(task.id).catch(() => {}) }}
      />
    </main>
  )
}
