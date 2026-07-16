import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { connectSSE, deleteTask, fetchSettings, fetchTasks, fetchUserscriptStatus, openExplorer, taskAction } from './api'
import { fmtBytes, fmtSpeed } from './format'
import { isRunningStatus, mergeTaskEvent } from './taskState'
import { commandState } from './taskCommands'
import { resolveTheme, type Theme } from './theme'
import type { Settings, Task, UserscriptStatus } from './types'
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

export default function App() {
  const [tasks, setTasks] = useState<Task[]>([])
  const [settings, setSettings] = useState<Settings>({})
  const [userscript, setUserscript] = useState<UserscriptStatus | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState<TaskFilter>('all')
  const [details, setDetails] = useState<Task | null>(null)
  const [logTaskId, setLogTaskId] = useState<string | null>(null)
  const [showRecognize, setShowRecognize] = useState(false)
  const [recognizeInitialUrl, setRecognizeInitialUrl] = useState('')
  const [showBatch, setShowBatch] = useState(false)
  const [showUserscript, setShowUserscript] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [showUpdate, setShowUpdate] = useState(false)
  const [error, setError] = useState('')
  const [theme, setTheme] = useState<Theme>(() => resolveTheme(localStorage.getItem('hls_theme'), matchMedia('(prefers-color-scheme: dark)').matches))
  const lastStatuses = useRef<Record<string, string>>({})

  const load = useCallback(async () => {
    try {
      const [taskData, settingData, scriptData] = await Promise.all([fetchTasks(), fetchSettings(), fetchUserscriptStatus()])
      setTasks(taskData); setSettings(settingData); setUserscript(scriptData); setError('')
    } catch (reason: any) { setError(reason.message || '无法连接本地下载服务') }
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
    return () => { events.close(); window.clearInterval(timer) }
  }, [load])

  useEffect(() => { document.documentElement.dataset.theme = theme }, [theme])
  useEffect(() => { setSelected(current => new Set([...current].filter(id => tasks.some(task => task.id === id)))) }, [tasks])

  const filtered = useMemo(() => tasks.filter(task => {
    if (filter === 'all') return true
    if (filter === 'running') return isRunningStatus(task.status) || task.status === 'queued'
    return task.status === filter
  }).sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || '')), [tasks, filter])
  const selectedTasks = tasks.filter(task => selected.has(task.id))
  const commands = commandState(selectedTasks)
  const running = tasks.filter(task => isRunningStatus(task.status))
  const totalSpeed = running.reduce((sum, task) => sum + (task.speed_bytes_per_sec || 0), 0)
  const completedSize = tasks.filter(task => task.status === 'done').reduce((sum, task) => sum + (task.downloaded_bytes || 0), 0)
  const queued = tasks.filter(task => task.status === 'queued').length

  const perform = async (action: string, targets: Task[] = selectedTasks) => {
    if (!targets.length) return
    if (action === 'delete' && !confirm(`确定删除 ${targets.length} 个任务？`)) return
    setError('')
    try {
      if (action === 'delete') await Promise.all(targets.map(task => deleteTask(task.id)))
      else await Promise.all(targets.map(task => taskAction(task.id, action)))
      setSelected(new Set()); await load()
    } catch (reason: any) { setError(reason.message || '任务操作失败') }
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
    <DesktopToolbar commands={commands} theme={theme} onNew={openRecognize} onPaste={pasteAndRecognize} onBatch={() => setShowBatch(true)} onAction={perform} onOpen={() => selectedTasks[0]?.output_path && openExplorer(selectedTasks[0].output_path)} onLog={() => setLogTaskId(selectedTasks[0]?.id || null)} onUserscript={() => setShowUserscript(true)} onRefresh={load} onUpdate={() => setShowUpdate(true)} onSettings={() => setShowSettings(true)} onToggleTheme={toggleTheme} />
    <div className="workspace">
      <Sidebar tasks={tasks} active={filter} onChange={setFilter} userscript={userscript} />
      <main className="content">
        <UpdateNotice />
        <div className="content-head"><strong>{filter === 'all' ? '全部任务' : '任务列表'} ({filtered.length})</strong><span>{selected.size ? `已选择 ${selected.size} 项` : '右键任务可直接操作，双击查看详情'}</span></div>
        {error && <div className="action-error">{error}</div>}
        <TaskTable tasks={filtered} selected={selected} onSelect={setSelected} onOpenDetails={setDetails} onTaskAction={(task, action) => perform(action, [task])} onOpenLog={task => setLogTaskId(task.id)} onOpenFile={task => task.output_path && openExplorer(task.output_path)} />
      </main>
    </div>
    <footer className="statusbar"><span>活动任务 <b>{running.length}</b></span><span>队列 <b>{queued}</b></span><span>总速度 <b>{fmtSpeed(totalSpeed)}</b></span><span>已完成 <b>{fmtBytes(completedSize)}</b></span><span>{userscript?.detected ? '油猴脚本已连接' : '本地服务正常'}</span></footer>
    {showRecognize && <RecognizeDialog settings={settings} initialUrl={recognizeInitialUrl} onClose={() => setShowRecognize(false)} onAdded={load} onNeedUserscript={() => { setShowRecognize(false); setShowUserscript(true) }} />}
    {showBatch && <div className="modal-overlay" onMouseDown={() => setShowBatch(false)}><section className="modal" onMouseDown={event => event.stopPropagation()}><header><div><h2>批量添加</h2><p>每行输入一个 m3u8 链接</p></div></header><BatchAddPanel settings={settings} onAdded={() => { setShowBatch(false); load() }} /><footer><button className="secondary-button" onClick={() => setShowBatch(false)}>关闭</button></footer></section></div>}
    {showUserscript && <UserscriptDialog onClose={() => { setShowUserscript(false); load() }} />}
    {showSettings && <SettingsPanel onClose={() => { setShowSettings(false); load() }} />}
    {showUpdate && <UpdateDialog onClose={() => setShowUpdate(false)} />}
    {details && <TaskDetailsModal task={tasks.find(task => task.id === details.id) || details} onClose={() => setDetails(null)} onLog={() => setLogTaskId(details.id)} />}
    {logTaskId && <LogModal taskId={logTaskId} onClose={() => setLogTaskId(null)} />}
  </div>
}
