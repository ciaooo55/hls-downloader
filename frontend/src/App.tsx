import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { FastForward, LoaderCircle, Pause, Play, Trash2, X } from 'lucide-react'
import { cancelPowerAction, castLocalFile, castMediaUrl, castTask, clearCompletedTasks, completeBrowserMediaPush, confirmPowerAction, connectSSE, controlCast, deleteTask, fetchBrowserHandoffs, fetchBrowserStatus, fetchHealth, fetchLegalStatus, fetchLocalTvboxShare, fetchPendingPowerActions, fetchSettings, fetchTasks, importTorrentPath, launchFile, openExplorer, openTaskInExplorer, pushLocalTvboxFile, pushTaskToTvbox, pushTvboxUrl, resolveBrowserHandoff, saveSettings, stopLocalTvboxShare, taskAction, taskFileUrl } from './api'
import { fmtBytes, fmtSpeed } from './format'
import { isRunningStatus, mergeTaskEvent, mergeTaskEvents } from './taskState'
import { commandState } from './taskCommands'
import { filterAndSortTasks } from './taskPresentation'
import type { ThemePreference } from './theme'
import type { BrowserStatus, LegalStatus, Settings, Task } from './types'
import DesktopToolbar from './components/DesktopToolbar'
import WindowChrome from './components/WindowChrome'
import Sidebar, { type TaskFilter } from './components/Sidebar'
import TaskTable from './components/TaskTable'
import TaskDetailsModal from './components/TaskDetailsModal'
import RecognizeDialog from './components/RecognizeDialog'
import BrowserExtensionDialog from './components/BrowserExtensionDialog'
import SettingsPanel from './components/SettingsPanel'
import BatchAddPanel from './components/BatchAddPanel'
import LogModal from './components/LogModal'
import UpdateNotice from './components/UpdateNotice'
import UpdateDialog from './components/UpdateDialog'
import BrowserHandoffDialog, { type BrowserHandoff, type BrowserHandoffCancelDecision, type BrowserHandoffDecision } from './components/BrowserHandoffDialog'
import ConfirmDialog from './components/ConfirmDialog'
import DevicePickerDialog from './components/DevicePickerDialog'
import MediaSourcePickerDialog, { type MediaSourceSelection } from './components/MediaSourcePickerDialog'
import LegalAgreementDialog from './components/LegalAgreementDialog'

const UI_EVENT_ID_CAP = 4096
import { Button, Dialog, DialogFooter, DialogHeader, DialogOverlay } from './components/ui'
import { isTauriDesktop, startTauriDesktopSession } from './tauri'
import { selectTheme, useUiStore } from './store/uiStore'
import { pickLocalMediaFile, quitApplication } from './desktop'

const VideoPlayerModal = lazy(() => import('./components/VideoPlayerModal'))
const launchParams = new URLSearchParams(window.location.search)

/** WebView2 has no Web Notification API, so the desktop shell must use the
 *  Tauri notification plugin; the web UI keeps the browser API. */
async function notifySystem(title: string, body: string): Promise<void> {
  try {
    if (isTauriDesktop()) {
      const plugin = await import('@tauri-apps/plugin-notification')
      let granted = await plugin.isPermissionGranted()
      if (!granted) granted = (await plugin.requestPermission()) === 'granted'
      if (granted) plugin.sendNotification({ title, body })
      return
    }
    if ('Notification' in window && Notification.permission === 'granted') {
      new Notification(title, { body })
    }
  } catch {
    // Notifications are optional; task state remains visible in the app.
  }
}

export default function App() {
  const [tasks, setTasks] = useState<Task[]>([])
  const [settings, setSettings] = useState<Settings>({})
  const [appVersion, setAppVersion] = useState('')
  const [legalStatus, setLegalStatus] = useState<LegalStatus | null>(null)
  const [legalLoadError, setLegalLoadError] = useState('')
  const [browserStatus, setBrowserStatus] = useState<BrowserStatus | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const filter = useUiStore(s => s.filter)
  const setFilter = useUiStore(s => s.setFilter)
  const query = useUiStore(s => s.query)
  const setQuery = useUiStore(s => s.setQuery)
  const themePreference = useUiStore(s => s.themePreference)
  const setThemePreference = useUiStore(s => s.setThemePreference)
  const systemDark = useUiStore(s => s.systemDark)
  const setSystemDark = useUiStore(s => s.setSystemDark)
  const toggleTheme = useUiStore(s => s.toggleTheme)
  const theme = useUiStore(selectTheme)
  const [pending, setPending] = useState<Set<string>>(new Set())
  const [feedback, setFeedback] = useState('')
  const [details, setDetails] = useState<Task | null>(null)
  const [logTaskId, setLogTaskId] = useState<string | null>(null)
  const [playing, setPlaying] = useState<Task | null>(null)
  const [previewImage, setPreviewImage] = useState<Task | null>(null)
  const [showRecognize, setShowRecognize] = useState(false)
  const [recognizeInitialUrl, setRecognizeInitialUrl] = useState('')
  const [showBatch, setShowBatch] = useState(false)
  const [showBrowserExtension, setShowBrowserExtension] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [showUpdate, setShowUpdate] = useState(false)
  const [localPushBusy, setLocalPushBusy] = useState(false)
  const [castBusy, setCastBusy] = useState(false)
  const [castControlBusy, setCastControlBusy] = useState(false)
  const [localShare, setLocalShare] = useState<{ id: string; filename: string; idleCleanupSeconds: number; kind: 'cast' | 'tvbox'; device?: object } | null>(null)
  const [handoffs, setHandoffs] = useState<BrowserHandoff[]>([])
  const [handoffBusy, setHandoffBusy] = useState(false)
  const [error, setError] = useState('')
  const [confirmation, setConfirmation] = useState<{ title: string; message: string; confirmLabel: string; danger: boolean; run: () => void } | null>(null)
  const [powerAction, setPowerAction] = useState<{ power_action_id: string; action: 'shutdown' | 'sleep' | 'hibernate'; task_title: string; delay_seconds: number } | null>(null)
  const [devicePick, setDevicePick] = useState<{ kind: 'cast' | 'tvbox'; path?: string; url?: string; taskId?: string; filename: string; requestId?: string } | null>(null)
  const [mediaSourcePick, setMediaSourcePick] = useState<{ kind: 'cast' | 'tvbox' } | null>(null)
  const [clipboardOffer, setClipboardOffer] = useState('')
  const [clipboardBatch, setClipboardBatch] = useState('')
  const [speedMenuOpen, setSpeedMenuOpen] = useState(false)
  const [batchInitialText, setBatchInitialText] = useState('')
  const lastStatuses = useRef<Record<string, string>>({})
  const feedbackTimer = useRef<number | null>(null)
  const clipboardOfferTimer = useRef<number | null>(null)
  const lastClipboardOffer = useRef('')
  const tasksRef = useRef<Task[]>([])
  const loadInFlight = useRef<Promise<void> | null>(null)
  const progressEventBatch = useRef<Map<string, Record<string, any>>>(new Map())
  const progressFlushTimer = useRef<number | null>(null)
  const deletedTaskIds = useRef<Set<string>>(new Set())
  const handoffRefreshInFlight = useRef(false)
  const autoPlayHandled = useRef(false)

  const loadLegal = useCallback(async () => {
    setLegalLoadError('')
    setLegalStatus(null)
    try {
      setLegalStatus(await fetchLegalStatus())
    } catch (reason: any) {
      setLegalLoadError(reason?.message || '无法读取本机协议状态')
    }
  }, [])

  useEffect(() => { void loadLegal() }, [loadLegal])

  useEffect(() => {
    if (!legalStatus?.accepted) return
    let disposed = false
    let stop: (() => void) | undefined
    void startTauriDesktopSession()
      .then(cleanup => {
        if (disposed) cleanup()
        else stop = cleanup
      })
      .catch(reason => {
        if (!disposed) setError(reason?.message || '无法启动桌面会话')
      })
    return () => {
      disposed = true
      stop?.()
    }
  }, [legalStatus?.accepted])

  useEffect(() => {
    const receive = (event: Event) => {
      const item = (event as CustomEvent).detail
      const resource = item?.resource || {}
      if ((item?.kind === 'cast' || item?.kind === 'tvbox') && resource.url) {
        setDevicePick({ kind: item.kind, url: String(resource.url), filename: String(resource.filename || resource.title || '网页视频'), requestId: String(item.id || '') || undefined })
      }
    }
    window.addEventListener('hls-browser-media-push', receive)
    return () => window.removeEventListener('hls-browser-media-push', receive)
  }, [])

  const load = useCallback(async () => {
    if (loadInFlight.current) return loadInFlight.current
    const request = (async () => { try {
      const [taskData, settingData, browserData, healthData, powerActions] = await Promise.all([fetchTasks(), fetchSettings(), fetchBrowserStatus(), fetchHealth(), fetchPendingPowerActions()])
      const pendingProgress = [...progressEventBatch.current.values()]
      progressEventBatch.current.clear()
      if (progressFlushTimer.current !== null) window.clearTimeout(progressFlushTimer.current)
      progressFlushTimer.current = null
      const visibleTasks = taskData.filter(task => !deletedTaskIds.current.has(task.id))
      setTasks(mergeTaskEvents(visibleTasks, pendingProgress, deletedTaskIds.current) as Task[]); setSettings(settingData); setBrowserStatus(browserData); setAppVersion(healthData.version || ''); setPowerAction(powerActions[0] || null); setError('')
      try {
        if ('Notification' in window && Notification.permission === 'default') {
          void Notification.requestPermission()
        }
      } catch {
        // Optional desktop notifications; list state remains authoritative.
      }
    } catch (reason: any) { setError(reason.message || '无法连接本地下载服务') } })()
    loadInFlight.current = request
    try { await request } finally { loadInFlight.current = null }
  }, [])

  useEffect(() => {
    load()
    const events = connectSSE(event => {
      if (event.type === 'task_deleted' && event.task_id) {
        deletedTaskIds.current.add(event.task_id)
        while (deletedTaskIds.current.size > UI_EVENT_ID_CAP) {
          const oldest = deletedTaskIds.current.values().next().value
          if (typeof oldest !== 'string') break
          deletedTaskIds.current.delete(oldest)
        }
      }
      if (event.type === 'power_action_pending' && event.power_action_id) {
        setPowerAction(event)
        void notifySystem('下载完成后的电源动作', `${event.delay_seconds || 30} 秒后将执行，可在下载器中取消。`)
      }
      if (['power_action_canceled', 'power_action_executed', 'power_action_failed'].includes(event.type)) {
        setPowerAction(current => current?.power_action_id === event.power_action_id ? null : current)
      }
      if (event.type === 'task_progress' && event.task_id) {
        const previous = lastStatuses.current[event.task_id]
        if (previous !== event.status) {
          lastStatuses.current[event.task_id] = event.status
          if (Object.keys(lastStatuses.current).length > UI_EVENT_ID_CAP) {
            const oldest = Object.keys(lastStatuses.current)[0]
            if (oldest) delete lastStatuses.current[oldest]
          }
          if (event.status === 'done') void notifySystem('下载完成', event.title || event.task_id)
          if (event.status === 'failed') void notifySystem('下载失败', event.error_message || event.task_id)
        }
        progressEventBatch.current.set(event.task_id, event)
        if (progressFlushTimer.current === null) {
          progressFlushTimer.current = window.setTimeout(() => {
            progressFlushTimer.current = null
            const batch = [...progressEventBatch.current.values()]
            progressEventBatch.current.clear()
            setTasks(previousTasks => mergeTaskEvents(
              previousTasks,
              batch,
              deletedTaskIds.current,
            ) as Task[])
          }, 100)
        }
      } else {
        setTasks(previous => mergeTaskEvent(previous, event, deletedTaskIds.current) as Task[])
      }
    }, load)
    const timer = window.setInterval(load, 30000)
    const onActivated = () => { void load() }
    window.addEventListener('desktop-activated', onActivated)
    return () => {
      events.close()
      window.clearInterval(timer)
      if (progressFlushTimer.current !== null) window.clearTimeout(progressFlushTimer.current)
      progressFlushTimer.current = null
      progressEventBatch.current.clear()
      window.removeEventListener('desktop-activated', onActivated)
    }
  }, [load])

  useEffect(() => {
    // Tauri owns dedicated handoff windows. The standalone /ui surface uses
    // the manager modal fallback instead.
    const desktopShell = isTauriDesktop()
    if (desktopShell || !legalStatus?.accepted) return
    const refresh = () => {
      if (handoffRefreshInFlight.current) return
      handoffRefreshInFlight.current = true
      void fetchBrowserHandoffs()
        .then(items => setHandoffs((items || []).filter(item => !item.status || item.status === 'pending')))
        .catch(() => {})
        .finally(() => { handoffRefreshInFlight.current = false })
    }
    refresh()
    const onVisible = () => { if (!document.hidden) refresh() }
    window.addEventListener('focus', refresh)
    document.addEventListener('visibilitychange', onVisible)
    const timer = window.setInterval(refresh, 1500)
    return () => {
      window.clearInterval(timer)
      window.removeEventListener('focus', refresh)
      document.removeEventListener('visibilitychange', onVisible)
    }
  }, [legalStatus?.accepted])

  useEffect(() => { document.documentElement.dataset.theme = theme }, [theme])
  useEffect(() => {
    const media = matchMedia('(prefers-color-scheme: dark)')
    const onChange = (event: MediaQueryListEvent) => setSystemDark(event.matches)
    setSystemDark(media.matches)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [])
  useEffect(() => {
    // Escape for surfaces owned directly by App (child modals handle themselves).
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      if (previewImage) { setPreviewImage(null); return }
      if (showBatch) { setShowBatch(false); setBatchInitialText('') }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [previewImage, showBatch])
  useEffect(() => { setSelected(current => new Set([...current].filter(id => tasks.some(task => task.id === id)))) }, [tasks])
  useEffect(() => {
    const requestedTask = launchParams.get('play')
    if (!legalStatus?.accepted || !requestedTask || playing || autoPlayHandled.current) return
    const task = tasks.find(item => item.id === requestedTask)
    if (task) { autoPlayHandled.current = true; setPlaying(task) }
  }, [tasks, playing, legalStatus?.accepted])
  useEffect(() => () => { if (feedbackTimer.current) window.clearTimeout(feedbackTimer.current) }, [])
  useEffect(() => { tasksRef.current = tasks }, [tasks])
  useEffect(() => {
    if (!speedMenuOpen) return
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') setSpeedMenuOpen(false) }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [speedMenuOpen])
  useEffect(() => {
    // IDM-style clipboard watching, desktop shell only: the Rust side emits
    // an event when copied text looks like a downloadable link.
    if (!legalStatus?.accepted || !isTauriDesktop() || settings.clipboard_watch === false) return
    let disposed = false
    const unlisteners: Array<() => void> = []
    void import('@tauri-apps/api/event')
      .then(async ({ listen }) => {
        const single = await listen<string>('clipboard-url', event => {
          const url = String(event.payload || '').trim()
          if (!url || url === lastClipboardOffer.current) return
          if (tasksRef.current.some(task => task.url === url)) return
          lastClipboardOffer.current = url
          setClipboardOffer(url)
          if (clipboardOfferTimer.current) window.clearTimeout(clipboardOfferTimer.current)
          clipboardOfferTimer.current = window.setTimeout(() => setClipboardOffer(''), 15_000)
        })
        const batch = await listen<string>('clipboard-url-batch', event => {
          const text = String(event.payload || '').trim()
          if (!text || text === lastClipboardOffer.current) return
          lastClipboardOffer.current = text
          setClipboardBatch(text)
          if (clipboardOfferTimer.current) window.clearTimeout(clipboardOfferTimer.current)
          clipboardOfferTimer.current = window.setTimeout(() => setClipboardBatch(''), 15_000)
        })
        if (disposed) { single(); batch() } else unlisteners.push(single, batch)
      })
      .catch(() => {})
    return () => {
      disposed = true
      for (const fn of unlisteners) fn()
      if (clipboardOfferTimer.current) window.clearTimeout(clipboardOfferTimer.current)
    }
  }, [settings.clipboard_watch, legalStatus?.accepted])
  useEffect(() => {
    if (!localShare) return
    let stopped = false
    const refresh = async () => {
      try {
        const status = await fetchLocalTvboxShare(localShare.id)
        if (!stopped && !status.active) {
          setLocalShare(null)
          showFeedback(`${localShare.kind === 'cast' ? '投屏' : 'TVBox 推送'}访问结束，已自动清理本机文件共享`)
        }
      } catch {
        // A transient local API failure should not falsely claim cleanup.
      }
    }
    const timer = window.setInterval(() => { void refresh() }, 20_000)
    return () => { stopped = true; window.clearInterval(timer) }
  }, [localShare])

  const filtered = useMemo(() => filterAndSortTasks(tasks, filter, query), [tasks, filter, query])
  const selectedTasks = tasks.filter(task => selected.has(task.id))
  const detailTask = details ? tasks.find(task => task.id === details.id) || details : null
  const playingTask = playing ? tasks.find(task => task.id === playing.id) || playing : null
  const commands = commandState(selectedTasks.some(task => pending.has(task.id)) ? [] : selectedTasks)
  const running = tasks.filter(task => isRunningStatus(task.status))
  const totalSpeed = running.reduce((sum, task) => sum + (task.speed_bytes_per_sec || 0), 0)
  const completedSize = tasks.filter(task => task.status === 'done').reduce((sum, task) => sum + (task.downloaded_bytes || 0), 0)
  const queued = tasks.filter(task => task.status === 'queued').length
  const completed = tasks.filter(task => task.status === 'done')

  const showFeedback = (message: string) => {
    setFeedback(message)
    if (feedbackTimer.current) window.clearTimeout(feedbackTimer.current)
    feedbackTimer.current = window.setTimeout(() => setFeedback(''), 3500)
  }

  const perform = async (action: string, targets: Task[] = selectedTasks, confirmed = false) => {
    if (!targets.length) return
    if (!confirmed && (action === 'delete' || action === 'deleteFiles')) {
      const deletesFiles = action === 'deleteFiles'
      setConfirmation({
        title: deletesFiles ? '删除任务和文件？' : '删除任务记录？',
        message: deletesFiles ? `将删除 ${targets.length} 个任务及其下载文件，此操作无法撤销。` : `将删除 ${targets.length} 条任务记录；未完成任务会停止并清理过程文件，已完成文件会保留。`,
        confirmLabel: deletesFiles ? '删除文件' : '删除记录',
        danger: true,
        run: () => { setConfirmation(null); void perform(action, targets, true) },
      })
      return
    }
    const fresh = targets.filter(task => !pending.has(task.id))
    if (!fresh.length) return
    const deleting = action === 'delete' || action === 'deleteFiles'
    if (deleting) {
      fresh.forEach(task => deletedTaskIds.current.add(task.id))
      setTasks(current => current.filter(task => !deletedTaskIds.current.has(task.id)))
    }
    setError('')
    setPending(current => new Set([...current, ...fresh.map(task => task.id)]))
    try {
      const apiAction = action.startsWith('queue_') ? `queue/${action.slice('queue_'.length)}` : action
      const results = await Promise.allSettled(fresh.map(task => action === 'delete' || action === 'deleteFiles' ? deleteTask(task.id, action === 'deleteFiles') : taskAction(task.id, apiAction)))
      const failures = results.filter(result => result.status === 'rejected') as PromiseRejectedResult[]
      if (deleting && failures.length) {
        results.forEach((result, index) => {
          if (result.status === 'rejected') deletedTaskIds.current.delete(fresh[index].id)
        })
      }
      const successCount = results.length - failures.length
      if (failures.length) {
        const reason = failures[0].reason
        setError(`成功 ${successCount} 项，失败 ${failures.length} 项：${reason?.message || '任务操作失败'}`)
      } else {
        showFeedback(`${action === 'delete' || action === 'deleteFiles' ? '已删除' : '操作完成'} ${successCount} 项`)
        setSelected(new Set())
      }
    } finally {
      setPending(current => new Set([...current].filter(id => !fresh.some(task => task.id === id))))
      await load()
    }
  }
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return
      if (!(event.key === 'Delete' || event.key === 'Backspace') || !selected.size) return
      if (selectedTasks.some(task => pending.has(task.id))) return
      event.preventDefault()
      void perform(event.shiftKey ? 'deleteFiles' : 'delete')
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [selected, selectedTasks, pending])

  const clearCompleted = async (confirmed = false) => {
    if (!completed.length) return
    if (!confirmed) {
      setConfirmation({
        title: '清理已完成记录？',
        message: `将从列表移除 ${completed.length} 条已完成记录，已下载文件不会被删除。`,
        confirmLabel: '清理记录',
        danger: false,
        run: () => { setConfirmation(null); void clearCompleted(true) },
      })
      return
    }
    try {
      const result = await clearCompletedTasks()
      showFeedback(`已清除 ${result.count} 条完成记录`)
      await load()
    } catch (reason: any) { setError(reason.message || '清理失败') }
  }
  const launchOutput = async (task: Task, confirmed = false) => {
    if (!task.output_path) return
    if (!confirmed && /\.(?:bat|cmd|com|exe|js|msi|ps1|scr|vbs)$/i.test(task.output_path)) {
      setConfirmation({
        title: '运行从互联网下载的文件？',
        message: `即将运行 ${task.filename || task.title || '可执行文件'}。仅在信任来源并已完成安全检查时继续。`,
        confirmLabel: '仍然运行',
        danger: true,
        run: () => { setConfirmation(null); void launchOutput(task, true) },
      })
      return
    }
    try {
      if (/\.torrent$/i.test(task.output_path)) { await importTorrentPath(task.output_path); showFeedback('已解析种子并创建 BT 下载任务'); await load(); return }
      await launchFile(task.id, confirmed)
    } catch (reason: any) { setError(reason.message || '无法打开文件') }
  }
  const resolveHandoff = async (action: 'accept' | 'cancel', decision?: BrowserHandoffDecision | BrowserHandoffCancelDecision) => {
    const item = handoffs[0]
    if (!item || handoffBusy) return
    setHandoffBusy(true)
    try {
      await resolveBrowserHandoff(item.id, action, decision)
      setHandoffs(current => current.filter(entry => entry.id !== item.id))
      if (action === 'accept') await load()
    } catch (reason: any) {
      setError(reason.message || '浏览器接管操作失败')
    } finally { setHandoffBusy(false) }
  }
  const changeThemePreference = (next: ThemePreference) => {
    setThemePreference(next)
  }
  const copyTaskUrl = async (task: Task) => {
    try {
      await navigator.clipboard.writeText(task.url || '')
      showFeedback('已复制下载链接')
    } catch {
      setError('无法复制链接，请手动选择地址栏文本')
    }
  }
  const pauseAllActive = async () => {
    const targets = tasks.filter(task => ['downloading', 'downloading_m3u8', 'downloading_segments', 'fetching_metadata', 'checking', 'parsing'].includes(task.status)
      || task.available_actions?.includes('pause'))
    if (!targets.length) { showFeedback('没有可暂停的任务'); return }
    await perform('pause', targets)
  }
  const startAllWaiting = async () => {
    const targets = tasks.filter(task => task.status === 'queued' || task.status === 'paused'
      || task.available_actions?.includes('start') || task.available_actions?.includes('resume'))
    if (!targets.length) { showFeedback('没有可开始的任务'); return }
    const resumes = targets.filter(task => task.status === 'paused' || task.available_actions?.includes('resume'))
    const starts = targets.filter(task => !resumes.includes(task))
    if (starts.length) await perform('start', starts)
    if (resumes.length) await perform('resume', resumes)
  }
  const applySpeedLimit = async (kib: number) => {
    try {
      const next = await saveSettings({ download_speed_limit_kib: kib })
      setSettings(next)
      showFeedback(kib > 0 ? `已限速 ${kib} KiB/s` : '已取消限速')
    } catch (reason: any) {
      setError(reason?.message || '设置限速失败')
    }
  }
  const openRecognize = () => { setRecognizeInitialUrl(''); setShowRecognize(true) }
  const pasteAndRecognize = async () => {
    try {
      setRecognizeInitialUrl((await navigator.clipboard.readText()).trim())
      setShowRecognize(true)
    } catch {
      setRecognizeInitialUrl('')
      setShowRecognize(true)
      setError('无法读取剪贴板，请在新建窗口中手动粘贴链接')
    }
  }
  const chooseLocalMedia = async (path?: string) => {
    let selectedPath = path || ''
    if (!selectedPath) {
      const result = await pickLocalMediaFile()
      if (result.canceled) return
      if (!result.ok || !result.path) { setError(result.error || '无法选择本机文件'); return }
      selectedPath = result.path
    }
    return { path: selectedPath, filename: selectedPath.split(/[\\/]/).pop() || selectedPath }
  }
  const confirmLocalMediaPush = async (taskOrPath?: Task | string) => {
    if (taskOrPath && typeof taskOrPath !== 'string' && taskOrPath.status !== 'done' && taskOrPath.playback_ready
      && ['http', 'torrent', 'hls', 'dash'].includes(taskOrPath.task_type || '')) {
      setDevicePick({ kind: 'tvbox', taskId: taskOrPath.id, filename: taskOrPath.filename || taskOrPath.title || taskOrPath.id })
      return
    }
    const selected = await chooseLocalMedia(typeof taskOrPath === 'string' ? taskOrPath : taskOrPath?.output_path)
    if (!selected) return
    setDevicePick({ kind: 'tvbox', path: selected.path, filename: selected.filename })
  }
  const confirmLocalCast = async (taskOrPath?: Task | string) => {
    if (taskOrPath && typeof taskOrPath !== 'string' && taskOrPath.status !== 'done' && taskOrPath.playback_ready
      && ['http', 'torrent', 'hls', 'dash'].includes(taskOrPath.task_type || '')) {
      setDevicePick({ kind: 'cast', taskId: taskOrPath.id, filename: taskOrPath.filename || taskOrPath.title || taskOrPath.id })
      return
    }
    const selected = await chooseLocalMedia(typeof taskOrPath === 'string' ? taskOrPath : taskOrPath?.output_path)
    if (!selected) return
    setDevicePick({ kind: 'cast', path: selected.path, filename: selected.filename })
  }
  const chooseDesktopMediaSource = (source: MediaSourceSelection) => {
    if (!mediaSourcePick) return
    const kind = mediaSourcePick.kind
    setMediaSourcePick(null)
    setDevicePick(source.source === 'url'
      ? { kind, url: source.url, filename: source.filename }
      : { kind, path: source.path, filename: source.filename })
  }
  const stopLocalShare = async () => {
    if (!localShare || localPushBusy || castBusy || castControlBusy) return
    setLocalPushBusy(true)
    try {
      if (localShare.id) await stopLocalTvboxShare(localShare.id)
      setLocalShare(null)
      showFeedback('已停止本机文件共享')
    } catch (reason: any) {
      setError(reason.message || '停止本机文件共享失败')
    } finally {
      setLocalPushBusy(false)
    }
  }
  const runCastControl = async (action: 'play' | 'pause' | 'seek') => {
    if (!localShare || localShare.kind !== 'cast' || castControlBusy || castBusy) return
    setCastControlBusy(true)
    setError('')
    try {
      const result = await controlCast(action, action === 'seek' ? 10 : 0, localShare.device)
      showFeedback(action === 'pause' ? `已暂停 ${result.label}` : action === 'play' ? `已继续播放 ${result.label}` : `已快进 10 秒：${result.label}`)
    } catch (reason: any) {
      setError(reason.message || '投屏控制失败')
    } finally {
      setCastControlBusy(false)
    }
  }

  const completeDevicePick = (device: any) => {
    if (!devicePick) return
    const pick = devicePick
    setDevicePick(null)
    void (async () => {
      const setBusy = pick.kind === 'cast' ? setCastBusy : setLocalPushBusy
      setBusy(true); setError('')
      try {
        if (pick.kind === 'cast') {
          if (pick.url) {
            const result = await castMediaUrl(pick.url, pick.filename, device)
            setLocalShare({ id: '', filename: pick.filename, idleCleanupSeconds: 0, kind: 'cast', device })
            showFeedback(`已投屏到 ${result.label}：${pick.filename}`)
          } else if (pick.path) {
            const result = await castLocalFile(pick.path, device)
            setLocalShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'cast', device })
            showFeedback(`已投屏到 ${result.label}：${pick.filename}`)
          } else if (pick.taskId) {
            const result = await castTask(pick.taskId, device)
            setLocalShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'cast', device })
            showFeedback(`已投屏当前下载到 ${result.label}：${pick.filename}`)
          }
        } else if (pick.url) {
          await pushTvboxUrl(pick.url, device.endpoint)
          showFeedback(`已 TVBox 推送：${pick.filename}`)
        } else if (pick.path) {
          const result = await pushLocalTvboxFile(pick.path, device.endpoint)
          setLocalShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'tvbox' })
          showFeedback(`已 TVBox 推送：${pick.filename}`)
        } else if (pick.taskId) {
          const result = await pushTaskToTvbox(pick.taskId, device.endpoint)
          setLocalShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'tvbox' })
          showFeedback(`已 TVBox 推送当前下载：${pick.filename}`)
        }
        if (pick.requestId) await completeBrowserMediaPush(pick.requestId, 'done', `已发送到 ${device.label || device.host || '所选设备'}`)
      } catch (reason: any) { setError(reason.message || '发送失败'); if (pick.requestId) void completeBrowserMediaPush(pick.requestId, 'failed', reason.message || '发送失败') }
      finally { setBusy(false) }
    })()
  }

  const desktopShell = isTauriDesktop()
  return <div className={`desktop-app${desktopShell ? ' has-window-chrome' : ''}`}>
    {desktopShell && <WindowChrome />}
    <DesktopToolbar commands={commands} theme={theme} version={appVersion} query={query} onQueryChange={setQuery} onNew={openRecognize} onPaste={pasteAndRecognize} onBatch={() => setShowBatch(true)} onAction={perform} onPauseAll={() => void pauseAllActive()} onStartAll={() => void startAllWaiting()} onOpen={() => selectedTasks[0]?.output_path && openTaskInExplorer(selectedTasks[0].id)} onLog={() => setLogTaskId(selectedTasks[0]?.id || null)} onBrowserExtension={() => setShowBrowserExtension(true)} onPushMedia={() => setMediaSourcePick({ kind: 'tvbox' })} pushLocalMediaBusy={localPushBusy} onCastMedia={() => setMediaSourcePick({ kind: 'cast' })} castLocalMediaBusy={castBusy} onRefresh={load} onUpdate={() => setShowUpdate(true)} onSettings={() => setShowSettings(true)} onToggleTheme={toggleTheme} />
    <div className="workspace">
      <Sidebar tasks={tasks} active={filter} onChange={setFilter} browserStatus={browserStatus} appVersion={appVersion} onOpenExtensionHelp={() => setShowBrowserExtension(true)} />
      <main className="content">
        <UpdateNotice />
        <div className="content-head"><strong>{filter === 'all' ? '全部任务' : filter === 'running' ? '进行中' : filter === 'done' ? '已完成' : filter === 'failed' ? '失败任务' : filter === 'media' ? '媒体' : filter === 'program' ? '程序' : filter === 'archive' ? '压缩包' : filter === 'other' ? '其他' : '任务列表'} <span>{filtered.length} 项{selected.size > 0 ? ` · 已选 ${selected.size}` : ''}</span></strong><button className="compact-button" disabled={!completed.length} title="只清除任务记录，不删除视频文件" onClick={() => void clearCompleted()}><Trash2 size={14} />清理已完成</button></div>
        {error && <div className="action-error" role="alert"><span>{error}</span><div className="action-error-actions"><button type="button" className="secondary-button" onClick={() => void load()}>重试</button><button type="button" className="icon-button action-error-dismiss" title="关闭提示" onClick={() => setError('')}><X size={15} /></button></div></div>}
    <TaskTable key={`${filter}:${query}`} tasks={filtered} selected={selected} pending={pending} onSelect={setSelected} onOpenDetails={setDetails} onTasksAction={(targets, action) => perform(action, targets)} onOpenLog={task => setLogTaskId(task.id)} onOpenFile={task => task.output_path && openTaskInExplorer(task.id)} onLaunchFile={launchOutput} onCopyUrl={task => void copyTaskUrl(task)} onPreview={setPlaying} onPreviewImage={setPreviewImage} onCast={task => void confirmLocalCast(task)} onPushToTv={task => void confirmLocalMediaPush(task)} />
      </main>
    </div>
    <footer className="statusbar">
      <span>活动任务 <b>{running.length}</b></span>
      <span>排队 <b>{queued}</b></span>
      <span>总速度 <b>{fmtSpeed(totalSpeed)}</b></span>
      <span className="speed-limit-control" title="全局下载限速">
        <button type="button" className="speed-limit-trigger" aria-label="全局下载限速" onClick={() => setSpeedMenuOpen(open => !open)}>
          限速 <b>{(settings.download_speed_limit_kib ?? 0) > 0 ? fmtSpeed((settings.download_speed_limit_kib ?? 0) * 1024) : '关'}</b>
        </button>
        {speedMenuOpen && <>
          <div className="floating-menu-backdrop" onMouseDown={() => setSpeedMenuOpen(false)} />
          <div className="floating-menu speed-limit-menu" role="menu">
            {[[0, '不限速'], [256, '256 KiB/s'], [512, '512 KiB/s'], [1024, '1 MiB/s'], [2048, '2 MiB/s'], [5120, '5 MiB/s'], [10240, '10 MiB/s']].map(([value, label]) => (
              <button key={value} role="menuitemradio" aria-checked={Number(settings.download_speed_limit_kib ?? 0) === value}
                onClick={() => { setSpeedMenuOpen(false); void applySpeedLimit(Number(value)) }}>
                <i>{Number(settings.download_speed_limit_kib ?? 0) === value ? '✓' : ''}</i>{label}
              </button>
            ))}
            <button role="menuitem" onClick={() => {
              setSpeedMenuOpen(false)
              const answer = window.prompt('自定义限速（KiB/s，0 表示不限速）', String(settings.download_speed_limit_kib ?? 0))
              if (answer === null) return
              const value = Math.max(0, Math.min(1048576, Math.round(Number(answer) || 0)))
              void applySpeedLimit(value)
            }}><i /> 自定义…</button>
          </div>
        </>}
      </span>
      <span>已完成 <b>{fmtBytes(completedSize)}</b></span>
      {localShare ? <span className="local-share-status" title={localShare.kind === 'cast' ? 'DLNA 投屏支持暂停、继续和快进；停止共享会立即取消本机媒体链接。' : 'TVBox 推送不定义通用播放控制；停止共享会立即取消本机媒体链接。'}><b>{localShare.kind === 'cast' ? '投屏共享中' : 'TVBox 共享中'}</b><em>{localShare.filename}</em>{localShare.kind === 'cast' && <span className="cast-controls"><button type="button" disabled={castControlBusy || castBusy} title="暂停投屏播放" aria-label="暂停投屏播放" onClick={() => void runCastControl('pause')}><Pause size={13} /></button><button type="button" disabled={castControlBusy || castBusy} title="继续投屏播放" aria-label="继续投屏播放" onClick={() => void runCastControl('play')}><Play size={13} /></button><button type="button" disabled={castControlBusy || castBusy} title="快进 10 秒" aria-label="快进 10 秒" onClick={() => void runCastControl('seek')}><FastForward size={13} /></button></span>}<button type="button" disabled={localPushBusy || castBusy || castControlBusy} onClick={() => void stopLocalShare()}>停止共享</button></span> : <span>{browserStatus?.detected ? `插件已连接${browserStatus.version ? ` · v${browserStatus.version}` : ''}` : `本地服务正常${appVersion ? ` · v${appVersion}` : ''}`}</span>}
    </footer>
    {showRecognize && <RecognizeDialog settings={settings} initialUrl={recognizeInitialUrl} onClose={() => setShowRecognize(false)} onAdded={task => { void load(); if (task?.task_type === 'torrent') setDetails(task) }} onNeedExtension={() => { setShowRecognize(false); setShowBrowserExtension(true) }} />}
    {showBatch && (
      <DialogOverlay onClose={() => { setShowBatch(false); setBatchInitialText('') }}>
        <Dialog className="batch-modal" label="批量添加" onClose={() => { setShowBatch(false); setBatchInitialText('') }}>
          <DialogHeader title="批量添加" description="每行一个链接：普通文件、HLS、DASH 或 magnet" onClose={() => { setShowBatch(false); setBatchInitialText('') }} />
          <BatchAddPanel key={batchInitialText || 'default'} settings={settings} initialText={batchInitialText} onAdded={() => { setShowBatch(false); setBatchInitialText(''); void load() }} />
          <DialogFooter>
            <Button variant="secondary" className="secondary-button" onClick={() => { setShowBatch(false); setBatchInitialText('') }}>关闭</Button>
          </DialogFooter>
        </Dialog>
      </DialogOverlay>
    )}
    {showBrowserExtension && <BrowserExtensionDialog onClose={() => { setShowBrowserExtension(false); load() }} />}
    {showSettings && <SettingsPanel themePreference={themePreference} onThemePreferenceChange={changeThemePreference} onClose={() => { setShowSettings(false); load() }} />}
    {showUpdate && <UpdateDialog onClose={() => setShowUpdate(false)} />}
    {mediaSourcePick && <MediaSourcePickerDialog mode={mediaSourcePick.kind} onChoose={chooseDesktopMediaSource} onClose={() => setMediaSourcePick(null)} />}
    {detailTask && <TaskDetailsModal task={detailTask} pending={pending.has(detailTask.id)} onClose={() => setDetails(null)} onLog={() => setLogTaskId(detailTask.id)} onAction={action => perform(action, [detailTask])} onOpenFile={() => detailTask.output_path && openTaskInExplorer(detailTask.id)} onLaunchFile={() => launchOutput(detailTask)} onPushToTv={() => void confirmLocalMediaPush(detailTask)} onCast={() => void confirmLocalCast(detailTask)} onPreview={() => { setDetails(null); setPlaying(detailTask) }} />}
    {playingTask && <Suspense fallback={<div className="modal-overlay player-overlay"><div className="player-chunk-loading"><LoaderCircle className="spin" size={24} /><span>正在打开播放器</span></div></div>}><VideoPlayerModal task={playingTask} onClose={() => setPlaying(null)} /></Suspense>}
    {previewImage && <div className="modal-overlay image-preview-overlay" onMouseDown={() => setPreviewImage(null)}><section className="image-preview" onMouseDown={event => event.stopPropagation()}><header><strong>{previewImage.title || previewImage.filename}</strong><button className="modal-close-button" title="关闭预览" onClick={() => setPreviewImage(null)}><X size={18} /></button></header><img src={taskFileUrl(previewImage.id, previewImage.file_access_token || '')} alt={previewImage.title || previewImage.filename} /></section></div>}
    {logTaskId && <LogModal taskId={logTaskId} onClose={() => setLogTaskId(null)} />}
    {feedback && <div className="toast" role="status">{feedback}</div>}
    {clipboardOffer && <div className="toast clipboard-toast" role="status">
      <span className="clipboard-toast-url" title={clipboardOffer}>检测到可下载链接：{clipboardOffer}</span>
      <button className="primary-button" onClick={() => { setRecognizeInitialUrl(clipboardOffer); setShowRecognize(true); setClipboardOffer('') }}>下载</button>
      <button className="secondary-button" onClick={() => setClipboardOffer('')}>忽略</button>
    </div>}
    {clipboardBatch && <div className="toast clipboard-toast" role="status">
      <span className="clipboard-toast-url">检测到 {clipboardBatch.split('\n').length} 条可下载链接</span>
      <button className="primary-button" onClick={() => { setBatchInitialText(clipboardBatch); setShowBatch(true); setClipboardBatch('') }}>批量导入</button>
      <button className="secondary-button" onClick={() => setClipboardBatch('')}>忽略</button>
    </div>}
    {handoffs[0] && <BrowserHandoffDialog key={handoffs[0].id} item={handoffs[0]} busy={handoffBusy} settings={settings} onResolve={resolveHandoff} queueRemaining={Math.max(0, handoffs.length - 1)} />}
    {confirmation && <ConfirmDialog title={confirmation.title} message={confirmation.message} confirmLabel={confirmation.confirmLabel} danger={confirmation.danger} onCancel={() => setConfirmation(null)} onConfirm={confirmation.run} />}
    {powerAction && <ConfirmDialog title={`${powerAction.task_title || '任务'} 已完成`} message={`${powerAction.delay_seconds || 30} 秒后将${powerAction.action === 'shutdown' ? '关机' : powerAction.action === 'sleep' ? '进入睡眠' : '进入休眠'}。可以立即执行或取消。`} confirmLabel="立即执行" danger={powerAction.action === 'shutdown'} onCancel={() => { const id = powerAction.power_action_id; setPowerAction(null); void cancelPowerAction(id).catch(() => {}) }} onConfirm={() => { const id = powerAction.power_action_id; setPowerAction(null); void confirmPowerAction(id).catch(reason => setError(reason?.message || '无法执行电源动作')) }} />}
    {devicePick && <DevicePickerDialog mode={devicePick.kind} onClose={() => { if (devicePick.requestId) void completeBrowserMediaPush(devicePick.requestId, 'canceled', '已取消设备选择'); setDevicePick(null) }} onChoose={completeDevicePick} />}
    {(!legalStatus || !legalStatus.accepted) && <LegalAgreementDialog
      status={legalStatus}
      required
      loadError={legalLoadError}
      onRetry={() => void loadLegal()}
      onAccepted={next => { setLegalStatus(next); setLegalLoadError(''); void load() }}
      onExit={() => { void quitApplication() }}
    />}
  </div>
}
