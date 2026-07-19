import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { FileText, FolderOpen, Info, LoaderCircle, MonitorPlay, MoreHorizontal, Pause, Play, PlayCircle, RotateCcw, Trash2, XCircle } from 'lucide-react'
import { getDisplayedProgress } from '../taskState'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import { taskContextActions, type TaskContextAction } from '../taskContextActions'
import { statusLabel } from '../taskPresentation'
import type { Task } from '../types'

const menuLabels: Record<TaskContextAction, string> = {
  details: '查看详情', start: '开始下载', pause: '暂停', resume: '恢复',
  cancel: '取消任务', retry: '重试', preview: '内置播放', launch: '系统播放', open: '打开文件位置', log: '查看日志', delete: '删除任务',
}

const menuIcons: Record<TaskContextAction, React.ReactNode> = {
  details: <Info size={16} />, start: <Play size={16} />, pause: <Pause size={16} />,
  resume: <RotateCcw size={16} />, cancel: <XCircle size={16} />, retry: <RotateCcw size={16} />,
  preview: <MonitorPlay size={16} />, launch: <PlayCircle size={16} />, open: <FolderOpen size={16} />, log: <FileText size={16} />, delete: <Trash2 size={16} />,
}

interface ContextMenuState { task: Task; x: number; y: number }

export default function TaskTable({ tasks, selected, pending, onSelect, onOpenDetails, onTaskAction, onOpenLog, onOpenFile, onLaunchFile, onPreview }: {
  tasks: Task[]
  selected: Set<string>
  pending: Set<string>
  onSelect: (ids: Set<string>) => void
  onOpenDetails: (task: Task) => void
  onTaskAction: (task: Task, action: string) => void
  onOpenLog: (task: Task) => void
  onOpenFile: (task: Task) => void
  onLaunchFile: (task: Task) => void
  onPreview: (task: Task) => void
}) {
  const [menu, setMenu] = useState<ContextMenuState | null>(null)
  const allSelected = tasks.length > 0 && tasks.every(task => selected.has(task.id))
  const toggleAll = () => onSelect(allSelected ? new Set() : new Set(tasks.map(task => task.id)))
  const toggleOne = (id: string) => {
    const next = new Set(selected)
    if (next.has(id)) next.delete(id); else next.add(id)
    onSelect(next)
  }

  useEffect(() => {
    if (!menu) return
    const close = () => setMenu(null)
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') close() }
    window.addEventListener('pointerdown', close)
    window.addEventListener('blur', close)
    window.addEventListener('resize', close)
    window.addEventListener('keydown', onKeyDown)
    document.addEventListener('scroll', close, true)
    return () => {
      window.removeEventListener('pointerdown', close)
      window.removeEventListener('blur', close)
      window.removeEventListener('resize', close)
      window.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('scroll', close, true)
    }
  }, [menu])

  const openMenu = (event: React.MouseEvent, task: Task) => {
    event.preventDefault()
    onSelect(new Set([task.id]))
    const actions = pending.has(task.id) ? ['details', 'log'] : taskContextActions(task)
    const width = 184
    const height = actions.length * 36 + 12
    setMenu({
      task,
      x: Math.max(6, Math.min(event.clientX, window.innerWidth - width - 6)),
      y: Math.max(6, Math.min(event.clientY, window.innerHeight - height - 6)),
    })
  }

  const runMenuAction = (action: TaskContextAction) => {
    if (!menu) return
    const task = menu.task
    setMenu(null)
    if (action === 'details') onOpenDetails(task)
    else if (action === 'preview') onPreview(task)
    else if (action === 'launch') onLaunchFile(task)
    else if (action === 'open') onOpenFile(task)
    else if (action === 'log') onOpenLog(task)
    else onTaskAction(task, action)
  }

  if (!tasks.length) return <div className="empty-state"><DownloadCloudIcon /><strong>暂无任务</strong><span>点击“新建”粘贴 m3u8 或普通网页链接</span></div>
  return (
    <div className="table-scroll">
      <table className="task-table">
        <thead><tr><th className="check-col"><input type="checkbox" checked={allSelected} onChange={toggleAll} aria-label="选择全部任务" /></th><th>文件名</th><th>状态</th><th>进度</th><th>速度 / 剩余</th><th className="segments-col">分片</th><th className="updated-col">更新时间</th><th className="menu-col" /></tr></thead>
        <tbody>{tasks.map(task => {
          const progress = getDisplayedProgress(task)
          return <tr key={task.id} className={`${selected.has(task.id) ? 'selected ' : ''}${pending.has(task.id) ? 'pending' : ''}`.trim()}
            onClick={event => {
              if ((event.target as HTMLElement).closest('input')) return
              if (event.ctrlKey || event.metaKey) toggleOne(task.id)
              else onSelect(new Set([task.id]))
            }}
            onContextMenu={event => openMenu(event, task)}
            onDoubleClick={() => task.available_actions?.includes('preview') ? onPreview(task) : onOpenDetails(task)}>
            <td className="check-col"><input type="checkbox" checked={selected.has(task.id)} onChange={() => toggleOne(task.id)} aria-label={`选择 ${task.title || task.filename || task.id}`} /></td>
            <td className="name-cell"><span title={task.title || task.filename || task.id}>{task.title || task.filename || task.id}</span><small title={task.url}>{task.url}</small></td>
            <td><span className={`status status-${task.status}`}>{pending.has(task.id) && <LoaderCircle className="spin" size={12} />}{task.status === 'queued' && task.queue_position ? `排队中 · 第 ${task.queue_position} 位` : statusLabel(task.status)}</span>{task.error_code && <small className="failure-code" title={task.error_message}>{task.error_code}</small>}</td>
            <td><div className="table-progress"><div><i style={{ width: `${Math.max(0, Math.min(100, progress))}%` }} /></div><span>{progress.toFixed(1)}%</span></div><small className="progress-bytes">{fmtBytes(task.downloaded_bytes)} / {fmtBytes(task.total_bytes)}</small></td>
            <td><span className="speed-cell">{fmtSpeed(task.speed_bytes_per_sec)}</span><small className="eta-cell">{fmtEta(task.eta_seconds)}</small></td>
            <td className="segments-col">{task.total_segments ? `${task.completed_segments}/${task.total_segments}` : '--'}</td><td className="updated-col">{fmtDate(task.updated_at)}</td>
            <td className="menu-col"><button className="row-menu-button" title="任务操作" onClick={event => { event.stopPropagation(); openMenu(event, task) }}><MoreHorizontal size={17} /></button></td>
          </tr>
        })}</tbody>
      </table>
      {menu && createPortal(
        <div className="task-context-menu" role="menu" style={{ left: menu.x, top: menu.y }} onPointerDown={event => event.stopPropagation()}>
          {(pending.has(menu.task.id) ? ['details', 'log'] as TaskContextAction[] : taskContextActions(menu.task)).map(action => <button key={action} role="menuitem" className={action === 'delete' ? 'danger' : ''} onClick={() => runMenuAction(action)}>
            {menuIcons[action]}<span>{action === 'preview' && menu.task.status !== 'done' ? '边下边播' : menuLabels[action]}</span>
          </button>)}
        </div>,
        document.body,
      )}
    </div>
  )
}

function DownloadCloudIcon() {
  return <div className="empty-glyph">↓</div>
}
