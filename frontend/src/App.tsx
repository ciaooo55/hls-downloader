import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { LoaderCircle, Search, Trash2, X } from 'lucide-react'
import { clearCompletedTasks, connectSSE, deleteTask, fetchBrowserHandoffs, fetchBrowserStatus, fetchHealth, fetchSettings, fetchTasks, fetchUserscriptStatus, launchFile, openExplorer, resolveBrowserHandoff, taskAction, taskFileUrl } from './api'
import { fmtBytes, fmtSpeed } from './format'
import { isRunningStatus, mergeTaskEvent } from './taskState'
import { commandState } from './taskCommands'
import { filterAndSortTasks } from './taskPresentation'
import { resolveTheme, type Theme } from './theme'
import type { BrowserStatus, Settings, Task, UserscriptStatus } from './types'
import DesktopToolbar from './components/DesktopToolbar'
import Sidebar, { type TaskFilter } from './components/Sidebar'
import TaskTable from './components/TaskTable'
import TaskDetailsModal from './components/TaskDetailsModal'
import RecognizeDialog from './components/RecognizeDialog'
import UserscriptDialog from './components/UserscriptDialog'
import SettingsPanel from './components/SettingsPanel'
import BatchAddPanel from './components/BatchAddPanel'
import LogModal from './components/LogModal'
import UpdateNotice from './components/UpdateNotice'
import UpdateDialog from './components/UpdateDialog'
import BrowserHandoffDialog, { type BrowserHandoff, type BrowserHandoffDecision } from './components/BrowserHandoffDialog'

const VideoPlayerModal = lazy(() => import('./components/VideoPlayerModal'))

export default function App() {
  const [tasks, setTasks] = useState<Task[]>([])
  const [settings, setSettings] = useState<Settings>({})
  const [appVersion, setAppVersion] = useState('')
  const [userscript, setUserscript] = useState<UserscriptStatus | null>(null)
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
  const [showUserscript, setShowUserscript] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [showUpdate, setShowUpdate] = useState(false)
  const [handoffs, setHandoffs] = useState<BrowserHandoff[]>([])
  const [handoffBusy, setHandoffBusy] = useState(false)
  const [error, setError] = useState('')
  const [theme, setTheme] = useState<Theme>(() => resolveTheme(localStorage.getItem('hls_theme'), matchMedia('(prefers-color-scheme: dark)').matches))
  const lastStatuses = useRef<Record<string, string>>({})
  const feedbackTimer = useRef<number | null>(null)
  const loadInFlight = useRef<Promise<void> | null>(null)
  const handoffRefreshInFlight = useRef(false)

  const load = useCallback(async () => {
    if (loadInFlight.current) return loadInFlight.current
    const request = (async () => { try {
      const [taskData, settingData, scriptData, browserData, healthData] = await Promise.all([fetchTasks(), fetchSettings(), fetchUserscriptStatus(), fetchBrowserStatus(), fetchHealth()])
      setTasks(taskData); setSettings(settingData); setUserscript(scriptData); setBrowserStatus(browserData); setAppVersion(healthData.version || ''); setError('')
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
    const desktopShell = Boolean((window as any).pywebview || (window as any).chrome?.webview)
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

  const perform = async (action: string, targets: Task[] = selectedTasks) => {
    if (!targets.length) return
    if (action === 'delete' && !confirm(`确定删除 ${targets.length} 条任务记录？未完成任务会停止并清理临时文件，已完成文件会保留。`)) return
    if (action === 'deleteFiles' && !confirm(`确定删除 ${targets.length} 个任务及其下载文件？此操作无法撤销。`)) return
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

  const clearCompleted = async () => {
    if (!completed.length || !confirm(`清除 ${completed.length} 条已完成记录？不会删除视频文件。`)) return
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
    localStorage.setItem('hls_theme', next); setTheme(next)
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
    <DesktopToolbar commands={commands} theme={theme} version={appVersion} onNew={openRecognize} onPaste={pasteAndRecognize} onBatch={() => setShowBatch(true)} onAction={perform} onOpen={() => selectedTasks[0]?.output_path && openExplorer(selectedTasks[0].output_path)} onLog={() => setLogTaskId(selectedTasks[0]?.id || null)} onUserscript={() => setShowUserscript(true)} onRefresh={load} onUpdate={() => setShowUpdate(true)} onSettings={() => setShowSettings(true)} onToggleTheme={toggleTheme} />
    <div className="workspace">
      <Sidebar tasks={tasks} active={filter} onChange={setFilter} userscript={userscript} browserStatus={browserStatus} />
      <main className="content">
        <UpdateNotice />
        <div className="content-head"><strong>{filter === 'all' ? '全部任务' : '任务列表'} ({filtered.length}){selected.size > 0 ? <span> · 已选 {selected.size}</span> : null}</strong><div className="list-tools"><label className="task-search"><Search size={14} /><input value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索任务、链接或错误码" aria-label="搜索任务" />{query ? <button type="button" className="task-search-clear" title="清除搜索" aria-label="清除搜索" onClick={() => setQuery('')}><X size={13} /></button> : null}</label><button className="compact-button" disabled={!completed.length} title="只清除任务记录，不删除视频文件" onClick={clearCompleted}><Trash2 size={14} />清理已完成</button></div></div>
        {error && <div className="action-error" role="alert"><span>{error}</span><div className="action-error-actions"><button type="button" className="secondary-button" onClick={() => void load()}>重试</button><button type="button" className="icon-button action-error-dismiss" title="关闭提示" onClick={() => setError('')}><X size={15} /></button></div></div>}
        <TaskTable tasks={filtered} selected={selected} pending={pending} onSelect={setSelected} onOpenDetails={setDetails} onTasksAction={(targets, action) => perform(action, targets)} onOpenLog={task => setLogTaskId(task.id)} onOpenFile={task => task.output_path && openExplorer(task.output_path)} onLaunchFile={launchOutput} onPreview={setPlaying} onPreviewImage={setPreviewImage} />
      </main>
    </div>
    <footer className="statusbar"><span>活动任务 <b>{running.length}</b></span><span>队列 <b>{queued}</b></span><span>总速度 <b>{fmtSpeed(totalSpeed)}</b></span><span>已完成 <b>{fmtBytes(completedSize)}</b></span><span>{browserStatus?.detected ? `扩展已连接${browserStatus.version ? ` · v${browserStatus.version}` : ''}` : userscript?.detected ? '后备脚本已连接' : `本地服务正常${appVersion ? ` · v${appVersion}` : ''}`}</span></footer>
    {showRecognize && <RecognizeDialog settings={settings} initialUrl={recognizeInitialUrl} onClose={() => setShowRecognize(false)} onAdded={load} onNeedUserscript={() => { setShowRecognize(false); setShowUserscript(true) }} />}
    {showBatch && <div className="modal-overlay" onMouseDown={() => setShowBatch(false)}><section className="modal" onMouseDown={event => event.stopPropagation()}><header><div><h2>批量添加</h2><p>每行输入一个 m3u8 链接</p></div></header><BatchAddPanel settings={settings} onAdded={() => { setShowBatch(false); load() }} /><footer><button className="secondary-button" onClick={() => setShowBatch(false)}>关闭</button></footer></section></div>}
    {showUserscript && <UserscriptDialog onClose={() => { setShowUserscript(false); load() }} />}
    {showSettings && <SettingsPanel onClose={() => { setShowSettings(false); load() }} />}
    {showUpdate && <UpdateDialog onClose={() => setShowUpdate(false)} />}
    {detailTask && <TaskDetailsModal task={detailTask} pending={pending.has(detailTask.id)} onClose={() => setDetails(null)} onLog={() => setLogTaskId(detailTask.id)} onAction={action => perform(action, [detailTask])} onOpenFile={() => detailTask.output_path && openExplorer(detailTask.output_path)} onLaunchFile={() => launchOutput(detailTask)} onPreview={() => { setDetails(null); setPlaying(detailTask) }} />}
    {playingTask && <Suspense fallback={<div className="modal-overlay player-overlay"><div className="player-chunk-loading"><LoaderCircle className="spin" size={24} /><span>正在打开播放器</span></div></div>}><VideoPlayerModal task={playingTask} onClose={() => setPlaying(null)} /></Suspense>}
    {previewImage && <div className="modal-overlay image-preview-overlay" onMouseDown={() => setPreviewImage(null)}><section className="image-preview" onMouseDown={event => event.stopPropagation()}><header><strong>{previewImage.title || previewImage.filename}</strong><button className="modal-close-button" title="关闭预览" onClick={() => setPreviewImage(null)}><X size={18} /></button></header><img src={taskFileUrl(previewImage.id)} alt={previewImage.title || previewImage.filename} /></section></div>}
    {logTaskId && <LogModal taskId={logTaskId} onClose={() => setLogTaskId(null)} />}
    {feedback && <div className="toast" role="status">{feedback}</div>}
    {handoffs[0] && <BrowserHandoffDialog key={handoffs[0].id} item={handoffs[0]} busy={handoffBusy} settings={settings} onResolve={resolveHandoff} queueRemaining={Math.max(0, handoffs.length - 1)} />}
  </div>
}
