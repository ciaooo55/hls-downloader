import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { LoaderCircle, Trash2, X } from 'lucide-react'
import { cancelPowerAction, castLocalFile, castMediaUrl, castTask, clearCompletedTasks, completeBrowserMediaPush, confirmPowerAction, connectSSE, controlCast, deleteTask, fetchBrowserHandoffs, fetchBrowserStatus, fetchHealth, fetchLegalStatus, fetchLocalTvboxShare, fetchPendingPowerActions, fetchSettings, fetchTasks, importLinkPath, importTorrentPath, launchFile, openExplorer, openTaskInExplorer, pushLocalTvboxFile, pushTaskToTvbox, pushTvboxUrl, resolveBrowserHandoff, saveSettings, saveTaskSiteProfile, stopLocalTvboxShare, reorderQueue, taskAction, taskFileUrl, uploadTorrent } from './api'
import { fmtBytes, fmtSpeed } from './format'
import { isActiveTransfer, isRunningStatus, mergeTaskEvent, mergeTaskEvents } from './taskState'
import { commandState } from './taskCommands'
import { emptyCastPlayback, mergeCastPlayback, shareActivityLabel, shareStopLabel, type CastPlaybackStatus, type LocalShareSession } from './castSession'
import { filterAndSortTasks, emptyTaskListCopy } from './taskPresentation'
import { effectiveSpeedLimitKib as localEffectiveSpeedLimitKib } from './speedSchedule'
import type { ThemePreference } from './theme'
import type { BrowserStatus, LegalStatus, Settings, Task } from './types'
import { downloadTextFile, formatTaskExport, parseUrlList } from './urlList'
import { isEditableDropTarget, payloadFromDataTransfer, planDroppedPayload } from './dropImport'
import { applyQueueReorder, isQueueReorderDrag } from './queueReorder'
import { playCompletionChime, setCompletionSoundEnabled } from './completionSound'
import SpeedChart from './components/SpeedChart'
import DesktopToolbar from './components/DesktopToolbar'
import WindowChrome from './components/WindowChrome'
import Sidebar, { taskFilterLabel, type TaskFilter } from './components/Sidebar'
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
import CastSessionHud from './components/CastSessionHud'
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
  const [localShare, setLocalShare] = useState<LocalShareSession | null>(null)
  const [hudMinimized, setHudMinimized] = useState(false)
  const [castPlayback, setCastPlayback] = useState<CastPlaybackStatus>(emptyCastPlayback())
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
  const [scheduleClock, setScheduleClock] = useState(() => Date.now())
  const [totalSpeedHistory, setTotalSpeedHistory] = useState<number[]>([])
  const totalSpeedRef = useRef(0)
  const castControlBusyRef = useRef(false)
  const [batchInitialText, setBatchInitialText] = useState('')
  const [batchInitialMode, setBatchInitialMode] = useState<'list' | 'harvest'>('list')
  const [dropActive, setDropActive] = useState(false)
  const lastStatuses = useRef<Record<string, string>>({})
  const feedbackTimer = useRef<number | null>(null)
  const clipboardOfferTimer = useRef<number | null>(null)
  const lastClipboardOffer = useRef('')
  const tasksRef = useRef<Task[]>([])
  const loadInFlight = useRef<Promise<void> | null>(null)
  const loadQueued = useRef(false)
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

  const load = useCallback(async (): Promise<void> => {
    if (loadInFlight.current) {
      // A refresh requested while another is in flight must not silently
      // reuse the older snapshot: it may predate the operation (pause/resume)
      // that triggered this call. Chain one follow-up fetch instead.
      if (!loadQueued.current) {
        loadQueued.current = true
        return loadInFlight.current.then((): Promise<void> => {
          loadQueued.current = false
          return load()
        })
      }
      return loadInFlight.current
    }
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
    setCompletionSoundEnabled(Boolean(settings.completion_sound_enabled))
  }, [settings.completion_sound_enabled])
  useEffect(() => {
    if (!settings.speed_schedule_enabled) return
    setScheduleClock(Date.now())
    let timer = 0
    const ms = 60_000 - (Date.now() % 60_000) + 50
    const first = window.setTimeout(function tick() {
      setScheduleClock(Date.now())
      timer = window.setTimeout(tick, 60_000)
    }, ms)
    return () => {
      window.clearTimeout(first)
      window.clearTimeout(timer)
    }
  }, [settings.speed_schedule_enabled, settings.speed_schedule_start, settings.speed_schedule_end, settings.speed_schedule_limit_kib])

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
          if (event.status === 'done') {
            void notifySystem('下载完成', event.title || event.task_id)
            playCompletionChime()
          }
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

  const applyDropPlan = async (plan: ReturnType<typeof planDroppedPayload>, fileBlobs?: File[]) => {
    if (plan.kind === 'none') { showFeedback('没有可导入的链接或种子'); return }
    if (plan.kind === 'recognize') { setRecognizeInitialUrl(plan.url); setShowRecognize(true); return }
    if (plan.kind === 'batch') { setBatchInitialMode('list'); setBatchInitialText(plan.urls.join('\n')); setShowBatch(true); return }
    let imported = 0
    for (const [index, item] of plan.items.entries()) {
      try {
        if (item.path && item.kind === 'torrent') await importTorrentPath(item.path)
        else if (item.path && item.kind === 'link') await importLinkPath(item.path)
        else if (fileBlobs?.[index] && item.kind === 'torrent') await uploadTorrent(fileBlobs[index])
        else if (fileBlobs?.[index] && item.kind === 'link') {
          const nested = planDroppedPayload({ text: await fileBlobs[index].text() })
          if (nested.kind === 'recognize' || nested.kind === 'batch') await applyDropPlan(nested)
          else continue
        } else continue
        imported += 1
      } catch (reason: any) {
        setError(reason?.message || '拖放导入失败')
      }
    }
    if (imported) { showFeedback('已导入 ' + imported + ' 个文件'); void load() }
  }

  useEffect(() => {
    const onDragOver = (event: DragEvent) => {
      if (isEditableDropTarget(event.target)) return
      const types = Array.from(event.dataTransfer?.types || [])
      if (isQueueReorderDrag(types)) return
      if (!types.length || !(types.includes('Files') || types.includes('text/uri-list') || types.includes('text/plain'))) return
      event.preventDefault()
      setDropActive(true)
    }
    const onDragLeave = (event: DragEvent) => {
      if (event.relatedTarget) return
      setDropActive(false)
    }
    const onDrop = (event: DragEvent) => {
      if (isEditableDropTarget(event.target)) return
      event.preventDefault()
      setDropActive(false)
      const payload = payloadFromDataTransfer(event.dataTransfer)
      void applyDropPlan(planDroppedPayload(payload), event.dataTransfer ? Array.from(event.dataTransfer.files) : undefined)
    }
    window.addEventListener('dragover', onDragOver)
    window.addEventListener('dragleave', onDragLeave)
    window.addEventListener('drop', onDrop)
    let stopTauri: (() => void) | undefined
    if (isTauriDesktop()) {
      void import('@tauri-apps/api/webview').then(async api => {
        const webview = api.getCurrentWebview()
        stopTauri = await webview.onDragDropEvent(event => {
          const kind = event.payload.type
          if (kind === 'enter' || kind === 'over') setDropActive(true)
          else if (kind === 'leave') setDropActive(false)
          else if (kind === 'drop') {
            setDropActive(false)
            const paths = event.payload.paths || []
            void applyDropPlan(planDroppedPayload({ files: paths.map(path => ({ name: path, path })) }))
          }
        })
      }).catch(() => {})
    }
    return () => {
      window.removeEventListener('dragover', onDragOver)
      window.removeEventListener('dragleave', onDragLeave)
      window.removeEventListener('drop', onDrop)
      stopTauri?.()
    }
  }, [])

  useEffect(() => {
    const timer = window.setInterval(() => {
      const current = totalSpeedRef.current
      setTotalSpeedHistory(previous => {
        if (current <= 0 && (previous.length === 0 || previous[previous.length - 1] === 0)) return previous
        return [...previous.slice(-59), current]
      })
    }, 1000)
    return () => window.clearInterval(timer)
  }, [])

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
      if (showBatch) { setShowBatch(false); setBatchInitialText(''); setBatchInitialMode('list') }
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
    // A direct URL cast has no local share id: there is no file share to
    // poll or auto-clean, and polling an empty id only produced errors that
    // kept the "共享中" state forever.
    if (!localShare || !localShare.id) return
    let stopped = false
    const refresh = async () => {
      try {
        const status = await fetchLocalTvboxShare(localShare.id)
        if (!stopped && !status.active) {
          setLocalShare(null)
          setCastPlayback(emptyCastPlayback())
          showFeedback(`${localShare.kind === 'cast' ? '投屏' : 'TVBox 推送'}访问结束，已自动清理本机文件共享`)
        }
      } catch {
        // A transient local API failure should not falsely claim cleanup.
      }
    }
    const timer = window.setInterval(() => { void refresh() }, 20_000)
    return () => { stopped = true; window.clearInterval(timer) }
  }, [localShare])
  useEffect(() => {
    if (!localShare || localShare.kind !== 'cast') return
    let stopped = false
    let inFlight = false
    const refresh = async () => {
      if (stopped || inFlight || castControlBusyRef.current) return
      inFlight = true
      try {
        const status = await controlCast('status', 0, localShare.device)
        if (!stopped) setCastPlayback(current => mergeCastPlayback(current, status))
      } catch {
        // A renderer that cannot report position still accepts play/pause.
      } finally {
        inFlight = false
      }
    }
    void refresh()
    const timer = window.setInterval(() => { void refresh() }, 1_000)
    return () => { stopped = true; window.clearInterval(timer) }
  }, [localShare])

  const filtered = useMemo(() => filterAndSortTasks(tasks, filter, query), [tasks, filter, query])
  const selectedTasks = tasks.filter(task => selected.has(task.id))
  const detailTask = details ? tasks.find(task => task.id === details.id) || details : null
  const playingTask = playing ? tasks.find(task => task.id === playing.id) || playing : null
  const commands = commandState(selectedTasks.some(task => pending.has(task.id)) ? [] : selectedTasks)
  const running = tasks.filter(task => isRunningStatus(task.status))
  // "pausing" and post-processing stages carry the last sampled rate; only
  // stages that are actually transferring belong in the status-bar total.
  const totalSpeed = running.reduce((sum, task) => (
    isActiveTransfer(task.status) ? sum + (task.speed_bytes_per_sec || 0) : sum
  ), 0)
  const completedSize = tasks.filter(task => task.status === 'done').reduce((sum, task) => sum + (task.downloaded_bytes || 0), 0)
  const queued = tasks.filter(task => task.status === 'queued').length
  const effectiveSpeedLimitKib = localEffectiveSpeedLimitKib(settings, new Date(scheduleClock))
  const emptyCopy = emptyTaskListCopy(filter, query, tasks.length)
  totalSpeedRef.current = totalSpeed
  const completed = tasks.filter(task => task.status === 'done')
  castControlBusyRef.current = castControlBusy

  const showFeedback = (message: string) => {
    setFeedback(message)
    if (feedbackTimer.current) window.clearTimeout(feedbackTimer.current)
    feedbackTimer.current = window.setTimeout(() => setFeedback(''), 3500)
  }


  const exportTaskUrls = (targets?: Task[]) => {
    const list = (targets && targets.length ? targets : (selectedTasks.length ? selectedTasks : filtered))
    const content = formatTaskExport(list)
    const count = parseUrlList(content).urls.length
    if (!count) { showFeedback('没有可导出的链接'); return }
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '')
    downloadTextFile(`hls-urls-${stamp}.txt`, content)
    showFeedback(`已导出 ${count} 条链接`)
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
      const results = await Promise.allSettled(fresh.map(task => action === 'delete' || action === 'deleteFiles' ? deleteTask(task.id, action === 'deleteFiles') : action === 'saveSiteProfile' ? saveTaskSiteProfile(task.id) : taskAction(task.id, apiAction)))
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
        showFeedback(`${action === 'delete' || action === 'deleteFiles' ? '已删除' : action === 'saveSiteProfile' ? '已保存站点规则' : '操作完成'} ${successCount} 项`)
        setSelected(new Set())
      }
    } finally {
      setPending(current => new Set([...current].filter(id => !fresh.some(task => task.id === id))))
      await load()
    }
  }
  const reorderQueuedTask = async (taskId: string, direction: string) => {
    if (pending.has(taskId)) return
    const placement = direction.startsWith('after:') ? 'after' : 'before'
    const targetId = direction.split(':').slice(1).join(':')
    if (targetId) {
      setTasks(current => applyQueueReorder(current, taskId, targetId, placement))
    }
    try {
      await reorderQueue(taskId, direction)
      await load()
    } catch (error: any) {
      setError(error?.message || '调整队列顺序失败')
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
    // Mirror the backend's pausable set: manifest download/parsing stages
    // reject pause and only produced "当前阶段不能暂停" failures in the batch.
    const targets = tasks.filter(task => ['downloading', 'downloading_segments', 'fetching_metadata', 'checking'].includes(task.status)
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
  const beginShare = (session: LocalShareSession) => {
    setLocalShare(session)
    setHudMinimized(false)
    setCastPlayback(emptyCastPlayback())
  }
  const stopLocalShare = async () => {
    if (!localShare || localPushBusy || castBusy || castControlBusy) return
    setLocalPushBusy(true)
    try {
      if (localShare.kind === 'cast' && localShare.device) {
        try { await controlCast('stop', 0, localShare.device) } catch { /* still revoke the local share */ }
      }
      if (localShare.id) await stopLocalTvboxShare(localShare.id)
      const hadShare = Boolean(localShare.id)
      setLocalShare(null)
      setCastPlayback(emptyCastPlayback())
      showFeedback(hadShare ? '已停止本机文件共享' : '已停止投屏播放')
    } catch (reason: any) {
      setError(reason.message || '停止本机文件共享失败')
    } finally {
      setLocalPushBusy(false)
    }
  }
  const runCastControl = async (action: 'play' | 'pause' | 'seek' | 'seek_to' | 'stop', seconds = 0) => {
    if (!localShare || localShare.kind !== 'cast' || castControlBusy || castBusy) return
    setCastControlBusy(true)
    setError('')
    try {
      const delta = action === 'seek' ? seconds || 10 : seconds
      const result = await controlCast(action, delta, localShare.device)
      setCastPlayback(current => {
        const next = mergeCastPlayback(current, result)
        if (action === 'play') return { ...next, playing: true, paused: false }
        if (action === 'pause') return { ...next, playing: false, paused: true }
        if (action === 'stop') return { ...emptyCastPlayback(), label: next.label }
        return next
      })
      if (action === 'pause') showFeedback(`已暂停 ${result.label}`)
      else if (action === 'play') showFeedback(`已继续播放 ${result.label}`)
      else if (action === 'seek') showFeedback(`${delta < 0 ? '已后退' : '已快进'} ${Math.abs(delta)} 秒：${result.label}`)
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
            beginShare({ id: '', filename: pick.filename, idleCleanupSeconds: 0, kind: 'cast', device })
            showFeedback(`已投屏到 ${result.label}：${pick.filename}`)
          } else if (pick.path) {
            const result = await castLocalFile(pick.path, device)
            beginShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'cast', device })
            showFeedback(`已投屏到 ${result.label}：${pick.filename}`)
          } else if (pick.taskId) {
            const result = await castTask(pick.taskId, device)
            beginShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'cast', device, taskId: pick.taskId })
            showFeedback(`已投屏当前下载到 ${result.label}：${pick.filename}`)
          }
        } else if (pick.url) {
          await pushTvboxUrl(pick.url, device.endpoint)
          beginShare({ id: '', filename: pick.filename, idleCleanupSeconds: 0, kind: 'tvbox', device })
          showFeedback(`已 TVBox 推送：${pick.filename}`)
        } else if (pick.path) {
          const result = await pushLocalTvboxFile(pick.path, device.endpoint)
          beginShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'tvbox', device })
          showFeedback(`已 TVBox 推送：${pick.filename}`)
        } else if (pick.taskId) {
          const result = await pushTaskToTvbox(pick.taskId, device.endpoint)
          beginShare({ id: result.share.id, filename: result.share.filename, idleCleanupSeconds: result.share.idle_cleanup_seconds, kind: 'tvbox', device, taskId: pick.taskId })
          showFeedback(`已 TVBox 推送当前下载：${pick.filename}`)
        }
        if (pick.requestId) await completeBrowserMediaPush(pick.requestId, 'done', `已发送到 ${device.label || device.host || '所选设备'}`)
      } catch (reason: any) { setError(reason.message || '发送失败'); if (pick.requestId) void completeBrowserMediaPush(pick.requestId, 'failed', reason.message || '发送失败') }
      finally { setBusy(false) }
    })()
  }

  const desktopShell = isTauriDesktop()
  return <div className={`desktop-app${desktopShell ? ' has-window-chrome' : ''}${dropActive ? ' is-drop-target' : ''}`}>
    {desktopShell && <WindowChrome />}{dropActive && <div className="drop-import-overlay" role="status">松开以添加下载</div>}
    <DesktopToolbar commands={commands} theme={theme} version={appVersion} query={query} onQueryChange={setQuery} onNew={openRecognize} onPaste={pasteAndRecognize} onBatch={() => { setBatchInitialMode('list'); setShowBatch(true) }} onHarvest={() => { setBatchInitialMode('harvest'); setShowBatch(true) }} onExportUrls={() => exportTaskUrls()} onAction={perform} onPauseAll={() => void pauseAllActive()} onStartAll={() => void startAllWaiting()} onOpen={() => selectedTasks[0]?.output_path && openTaskInExplorer(selectedTasks[0].id)} onLog={() => setLogTaskId(selectedTasks[0]?.id || null)} onBrowserExtension={() => setShowBrowserExtension(true)} onPushMedia={() => setMediaSourcePick({ kind: 'tvbox' })} pushLocalMediaBusy={localPushBusy} onCastMedia={() => setMediaSourcePick({ kind: 'cast' })} castLocalMediaBusy={castBusy} onRefresh={load} onUpdate={() => setShowUpdate(true)} onSettings={() => setShowSettings(true)} onToggleTheme={toggleTheme} />
    <div className="workspace">
      <Sidebar tasks={tasks} active={filter} onChange={setFilter} browserStatus={browserStatus} appVersion={appVersion} onOpenExtensionHelp={() => setShowBrowserExtension(true)} />
      <main className="content">
        <UpdateNotice />
        <div className="content-head"><strong>{taskFilterLabel(filter)} <span>{filtered.length} 项{selected.size > 0 ? ` · 已选 ${selected.size}` : ''}</span></strong><button className="compact-button" disabled={!completed.length} title="只清除任务记录，不删除视频文件" onClick={() => void clearCompleted()}><Trash2 size={14} />清理已完成</button></div>
        {error && <div className="action-error" role="alert"><span>{error}</span><div className="action-error-actions"><button type="button" className="secondary-button" onClick={() => void load()}>重试</button><button type="button" className="icon-button action-error-dismiss" title="关闭提示" onClick={() => setError('')}><X size={15} /></button></div></div>}
    <TaskTable key={`${filter}:${query}`} tasks={filtered} selected={selected} pending={pending} emptyTitle={emptyCopy.title} emptyHint={emptyCopy.hint} onSelect={setSelected} onOpenDetails={setDetails} onTasksAction={(targets, action) => perform(action, targets)} onOpenLog={task => setLogTaskId(task.id)} onOpenFile={task => task.output_path && openTaskInExplorer(task.id)} onLaunchFile={launchOutput} onCopyUrl={task => void copyTaskUrl(task)} onExportUrls={exportTaskUrls} onPreview={setPlaying} onPreviewImage={setPreviewImage} onCast={task => void confirmLocalCast(task)} onPushToTv={task => void confirmLocalMediaPush(task)} onReorderQueue={reorderQueuedTask} />
      </main>
    </div>
    <footer className="statusbar">
      <span>活动任务 <b>{running.length}</b></span>
      <span>排队 <b>{queued}</b></span>
      <span className="total-speed-status">总速度 <b>{fmtSpeed(totalSpeed)}</b><SpeedChart samples={totalSpeedHistory} current={totalSpeed} compact /></span>
      <span className="speed-limit-control" title={settings.speed_schedule_enabled ? '当前生效限速（分时段已开启）' : '全局下载限速'}>
        <button type="button" className="speed-limit-trigger" aria-label="全局下载限速" onClick={() => setSpeedMenuOpen(open => !open)}>
          限速 <b>{effectiveSpeedLimitKib > 0 ? fmtSpeed(effectiveSpeedLimitKib * 1024) : '关'}</b>{settings.speed_schedule_enabled ? ' · 时段' : ''}
        </button>
        {speedMenuOpen && <>
          <div className="floating-menu-backdrop" onMouseDown={() => setSpeedMenuOpen(false)} />
          <div className="floating-menu speed-limit-menu" role="menu">
            {settings.speed_schedule_enabled ? <p className="speed-limit-menu-note">勾选改的是基础限速；按钮显示当前时段生效值</p> : null}
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
      {localShare ? <span className="local-share-status" title={localShare.kind === 'cast' ? '点击打开投屏悬浮窗，可暂停、拖动进度和停止。' : '点击打开推送悬浮窗；TVBox 播放由电视端控制，本机可停止。'}><button type="button" className="local-share-chip" onClick={() => setHudMinimized(false)}><b>{shareActivityLabel(localShare)}</b><em>{localShare.filename}</em></button><button type="button" disabled={localPushBusy || castBusy || castControlBusy} onClick={() => void stopLocalShare()}>{shareStopLabel(localShare)}</button></span> : <span>{browserStatus?.detected ? `插件已连接${browserStatus.version ? ` · v${browserStatus.version}` : ''}` : `本地服务正常${appVersion ? ` · v${appVersion}` : ''}`}</span>}
    </footer>
    {showRecognize && <RecognizeDialog settings={settings} initialUrl={recognizeInitialUrl} onClose={() => setShowRecognize(false)} onAdded={task => { void load(); if (task?.task_type === 'torrent') setDetails(task) }} onNeedExtension={() => { setShowRecognize(false); setShowBrowserExtension(true) }} />}
    {showBatch && (
      <DialogOverlay onClose={() => { setShowBatch(false); setBatchInitialText(''); setBatchInitialMode('list') }}>
        <Dialog className="batch-modal" label="批量添加" onClose={() => { setShowBatch(false); setBatchInitialText(''); setBatchInitialMode('list') }}>
          <DialogHeader title="批量添加" description="粘贴链接列表，或从当前网页抓取可下载文件" onClose={() => { setShowBatch(false); setBatchInitialText(''); setBatchInitialMode('list') }} />
          <BatchAddPanel key={`${batchInitialMode}:${batchInitialText || 'default'}`} settings={settings} initialText={batchInitialText} initialMode={batchInitialMode} onAdded={() => { setShowBatch(false); setBatchInitialText(''); setBatchInitialMode('list'); void load() }} />
          <DialogFooter>
            <Button variant="secondary" className="secondary-button" onClick={() => { setShowBatch(false); setBatchInitialText(''); setBatchInitialMode('list') }}>关闭</Button>
          </DialogFooter>
        </Dialog>
      </DialogOverlay>
    )}
    {showBrowserExtension && <BrowserExtensionDialog onClose={() => { setShowBrowserExtension(false); load() }} />}
    {showSettings && <SettingsPanel themePreference={themePreference} onThemePreferenceChange={changeThemePreference} onClose={() => { setShowSettings(false); load() }} />}
    {showUpdate && <UpdateDialog onClose={() => setShowUpdate(false)} />}
    {localShare && <CastSessionHud
      share={localShare}
      task={localShare.taskId ? tasks.find(item => item.id === localShare.taskId) || null : null}
      playback={castPlayback}
      busy={castControlBusy || castBusy || localPushBusy}
      minimized={hudMinimized}
      onMinimize={() => setHudMinimized(true)}
      onRestore={() => setHudMinimized(false)}
      onControl={(action, seconds) => void runCastControl(action, seconds)}
      onSeekTo={seconds => void runCastControl('seek_to', seconds)}
      onStop={() => void stopLocalShare()}
      onPauseDownload={() => {
        const task = localShare.taskId ? tasks.find(item => item.id === localShare.taskId) : null
        if (task) void perform('pause', [task])
      }}
      onResumeDownload={() => {
        const task = localShare.taskId ? tasks.find(item => item.id === localShare.taskId) : null
        if (task) void perform('resume', [task])
      }}
    />}
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
