import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { LoaderCircle, Trash2, X } from 'lucide-react'
import { clearCompletedTasks, connectSSE, deleteTask, fetchBrowserHandoffs, fetchBrowserStatus, fetchHealth, fetchSettings, fetchTasks, launchFile, openExplorer, resolveBrowserHandoff, taskAction, taskFileUrl } from './api'
import { fmtBytes, fmtSpeed } from './format'
import { isRunningStatus, mergeTaskEvent } from './taskState'
import { commandState } from './taskCommands'
import { filterAndSortTasks } from './taskPresentation'
import { resolveTheme, resolveThemePreference, type ThemePreference } from './theme'
import type { BrowserStatus, Settings, Task } from './types'
import DesktopToolbar from './components/DesktopToolbar'
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
import BrowserHandoffDialog, { type BrowserHandoff, type BrowserHandoffDecision } from './components/BrowserHandoffDialog'
import ConfirmDialog from './components/ConfirmDialog'
import { isTauriDesktop, startTauriDesktopSession } from './tauri'

const VideoPlayerModal = lazy(() => import('./components/VideoPlayerModal'))
const launchParams = new URLSearchParams(window.location.search)
const launchToken = launchParams.get('token')
if (launchToken) localStorage.setItem('hls_token', launchToken)

export default function App() {
  const [tasks, setTasks] = useState<Task[]>([])
  const [settings, setSettings] = useState<Settings>({})
  const [appVersion, setAppVersion] = useState('')
  const [browserStatus, setBrowserStatus] = useState<BrowserStatus | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState<TaskFilter>('all')
  const [query, setQuery] = useState('')
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
  const [handoffs, setHandoffs] = useState<BrowserHandoff[]>([])
  const [handoffBusy, setHandoffBusy] = useState(false)
  const [error, setError] = useState('')
  const [confirmation, setConfirmation] = useState<{ title: string; message: string; confirmLabel: string; danger: boolean; run: () => void } | null>(null)
  const [themePreference, setThemePreference] = useState<ThemePreference>(() => resolveThemePreference(localStorage.getItem('hls_theme')))
  const [systemDark, setSystemDark] = useState(() => matchMedia('(prefers-color-scheme: dark)').matches)
  const theme = resolveTheme(themePreference, systemDark)
  const lastStatuses = useRef<Record<string, string>>({})
  const feedbackTimer = useRef<number | null>(null)
  const loadInFlight = useRef<Promise<void> | null>(null)
  const handoffRefreshInFlight = useRef(false)
  const autoPlayHandled = useRef(false)

  useEffect(() => {
    let stop: (() => void) | undefined
    void startTauriDesktopSession()
      .then(cleanup => { stop = cleanup })
      .catch(reason => setError(reason?.message || '无法启动桌面会话'))
    return () => stop?.()
  }, [])

  const load = useCallback(async () => {
    if (loadInFlight.current) return loadInFlight.current
    const request = (async () => { try {
      const [taskData, settingData, browserData, healthData] = await Promise.all([fetchTasks(), fetchSettings(), fetchBrowserStatus(), fetchHealth()])
      setTasks(taskData); setSettings(settingData); setBrowserStatus(browserData); setAppVersion(healthData.version || ''); setError('')
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
      setTasks(previous => mergeTaskEvent(previous, event) as Task[])
      if (event.type === 'task_progress' && event.task_id) {
        const previous = lastStatuses.current[event.task_id]
        if (previous !== event.status) {
          lastStatuses.current[event.task_id] = event.status
          try {
            if ('Notification' in window && Notification.permission === 'granted') {
              if (event.status === 'done') new Notification('下载完成', { body: event.title || event.task_id })
              if (event.status === 'failed') new Notification('下载失败', { body: event.error_message || event.task_id })
            }
          } catch {
            // Desktop notifications are optional; task state remains visible in the app.
          }
        }
      }
    }, load)
    const timer = window.setInterval(load, 30000)
    const onActivated = () => { void load() }
    window.addEventListener('desktop-activated', onActivated)
    return () => {
      events.close()
      window.clearInterval(timer)
      window.removeEventListener('desktop-activated', onActivated)
    }
  }, [load])

  useEffect(() => {
    // Desktop owns dedicated handoff windows. Only pure /ui (no pywebview) needs the manager modal fallback.
    const desktopShell = Boolean((window as any).pywebview || (window as any).chrome?.webview || isTauriDesktop())
    if (desktopShell) return
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
  }, [])

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
      if (showBatch) { setShowBatch(false) }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [previewImage, showBatch])
  useEffect(() => { setSelected(current => new Set([...current].filter(id => tasks.some(task => task.id === id)))) }, [tasks])
  useEffect(() => {
    const requestedTask = launchParams.get('play')
    if (!requestedTask || playing || autoPlayHandled.current) return
    const task = tasks.find(item => item.id === requestedTask)
    if (task) { autoPlayHandled.current = true; setPlaying(task) }
  }, [tasks, playing])
  useEffect(() => () => { if (feedbackTimer.current) window.clearTimeout(feedbackTimer.current) }, [])

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
    setError('')
    setPending(current => new Set([...current, ...fresh.map(task => task.id)]))
    try {
      const results = await Promise.allSettled(fresh.map(task => action === 'delete' || action === 'deleteFiles' ? deleteTask(task.id, action === 'deleteFiles') : taskAction(task.id, action)))
      const failures = results.filter(result => result.status === 'rejected') as PromiseRejectedResult[]
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
  const launchOutput = async (task: Task) => {
    if (!task.output_path) return
    try { await launchFile(task.output_path) } catch (reason: any) { setError(reason.message || '无法打开文件') }
  }
  const resolveHandoff = async (action: 'accept' | 'cancel', decision?: BrowserHandoffDecision) => {
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
  const toggleTheme = () => {
    const next = theme === 'dark' ? 'light' : 'dark'
    localStorage.setItem('hls_theme', next); setThemePreference(next)
  }
  const changeThemePreference = (next: ThemePreference) => {
    localStorage.setItem('hls_theme', next)
    setThemePreference(next)
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

  return <div className="desktop-app">
    {isTauriDesktop() && <div className="tauri-drag-region" data-tauri-drag-region />}
    <DesktopToolbar commands={commands} theme={theme} version={appVersion} query={query} onQueryChange={setQuery} onNew={openRecognize} onPaste={pasteAndRecognize} onBatch={() => setShowBatch(true)} onAction={perform} onOpen={() => selectedTasks[0]?.output_path && openExplorer(selectedTasks[0].output_path)} onLog={() => setLogTaskId(selectedTasks[0]?.id || null)} onBrowserExtension={() => setShowBrowserExtension(true)} onRefresh={load} onUpdate={() => setShowUpdate(true)} onSettings={() => setShowSettings(true)} onToggleTheme={toggleTheme} />
    <div className="workspace">
      <Sidebar tasks={tasks} active={filter} onChange={setFilter} browserStatus={browserStatus} />
      <main className="content">
        <UpdateNotice />
        <div className="content-head"><strong>{filter === 'all' ? '全部任务' : '任务列表'} <span>{filtered.length} 项{selected.size > 0 ? ` · 已选 ${selected.size}` : ''}</span></strong><button className="compact-button" disabled={!completed.length} title="只清除任务记录，不删除视频文件" onClick={() => void clearCompleted()}><Trash2 size={14} />清理已完成</button></div>
        {error && <div className="action-error" role="alert"><span>{error}</span><div className="action-error-actions"><button type="button" className="secondary-button" onClick={() => void load()}>重试</button><button type="button" className="icon-button action-error-dismiss" title="关闭提示" onClick={() => setError('')}><X size={15} /></button></div></div>}
        <TaskTable tasks={filtered} selected={selected} pending={pending} onSelect={setSelected} onOpenDetails={setDetails} onTasksAction={(targets, action) => perform(action, targets)} onOpenLog={task => setLogTaskId(task.id)} onOpenFile={task => task.output_path && openExplorer(task.output_path)} onLaunchFile={launchOutput} onPreview={setPlaying} onPreviewImage={setPreviewImage} />
      </main>
    </div>
    <footer className="statusbar"><span>活动任务 <b>{running.length}</b></span><span>队列 <b>{queued}</b></span><span>总速度 <b>{fmtSpeed(totalSpeed)}</b></span><span>已完成 <b>{fmtBytes(completedSize)}</b></span><span>{browserStatus?.detected ? `插件已连接${browserStatus.version ? ` · v${browserStatus.version}` : ''}` : `本地服务正常${appVersion ? ` · v${appVersion}` : ''}`}</span></footer>
    {showRecognize && <RecognizeDialog settings={settings} initialUrl={recognizeInitialUrl} onClose={() => setShowRecognize(false)} onAdded={load} onNeedExtension={() => { setShowRecognize(false); setShowBrowserExtension(true) }} />}
    {showBatch && <div className="modal-overlay" onMouseDown={() => setShowBatch(false)}><section className="modal" onMouseDown={event => event.stopPropagation()}><header><div><h2>批量添加</h2><p>每行输入一个 m3u8 链接</p></div></header><BatchAddPanel settings={settings} onAdded={() => { setShowBatch(false); load() }} /><footer><button className="secondary-button" onClick={() => setShowBatch(false)}>关闭</button></footer></section></div>}
    {showBrowserExtension && <BrowserExtensionDialog onClose={() => { setShowBrowserExtension(false); load() }} />}
    {showSettings && <SettingsPanel themePreference={themePreference} onThemePreferenceChange={changeThemePreference} onClose={() => { setShowSettings(false); load() }} />}
    {showUpdate && <UpdateDialog onClose={() => setShowUpdate(false)} />}
    {detailTask && <TaskDetailsModal task={detailTask} pending={pending.has(detailTask.id)} onClose={() => setDetails(null)} onLog={() => setLogTaskId(detailTask.id)} onAction={action => perform(action, [detailTask])} onOpenFile={() => detailTask.output_path && openExplorer(detailTask.output_path)} onLaunchFile={() => launchOutput(detailTask)} onPreview={() => { setDetails(null); setPlaying(detailTask) }} />}
    {playingTask && <Suspense fallback={<div className="modal-overlay player-overlay"><div className="player-chunk-loading"><LoaderCircle className="spin" size={24} /><span>正在打开播放器</span></div></div>}><VideoPlayerModal task={playingTask} onClose={() => setPlaying(null)} /></Suspense>}
    {previewImage && <div className="modal-overlay image-preview-overlay" onMouseDown={() => setPreviewImage(null)}><section className="image-preview" onMouseDown={event => event.stopPropagation()}><header><strong>{previewImage.title || previewImage.filename}</strong><button className="modal-close-button" title="关闭预览" onClick={() => setPreviewImage(null)}><X size={18} /></button></header><img src={taskFileUrl(previewImage.id)} alt={previewImage.title || previewImage.filename} /></section></div>}
    {logTaskId && <LogModal taskId={logTaskId} onClose={() => setLogTaskId(null)} />}
    {feedback && <div className="toast" role="status">{feedback}</div>}
    {handoffs[0] && <BrowserHandoffDialog key={handoffs[0].id} item={handoffs[0]} busy={handoffBusy} settings={settings} onResolve={resolveHandoff} queueRemaining={Math.max(0, handoffs.length - 1)} />}
    {confirmation && <ConfirmDialog title={confirmation.title} message={confirmation.message} confirmLabel={confirmation.confirmLabel} danger={confirmation.danger} onCancel={() => setConfirmation(null)} onConfirm={confirmation.run} />}
  </div>
}
