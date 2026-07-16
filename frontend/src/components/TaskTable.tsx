import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { FileText, FolderOpen, Info, Pause, Play, RotateCcw, Trash2, XCircle } from 'lucide-react'
import { getDisplayedProgress } from '../taskState'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import { taskContextActions, type TaskContextAction } from '../taskContextActions'
import type { Task } from '../types'

const labels: Record<string, string> = {
  queued: '排队中', downloading: '下载中', downloading_m3u8: '获取清单', parsing: '解析中',
  downloading_segments: '下载分片', pausing: '暂停中', paused: '已暂停', merging: '合并中',
  remuxing: '转封装', done: '已完成', failed: '失败', canceled: '已取消', unsupported: '不支持',
}

const menuLabels: Record<TaskContextAction, string> = {
  details: '查看详情', start: '开始下载', pause: '暂停', resume: '恢复',
  cancel: '取消任务', retry: '重试', open: '打开文件位置', log: '查看日志', delete: '删除任务',
}

const menuIcons: Record<TaskContextAction, React.ReactNode> = {
  details: <Info size={16} />, start: <Play size={16} />, pause: <Pause size={16} />,
  resume: <RotateCcw size={16} />, cancel: <XCircle size={16} />, retry: <RotateCcw size={16} />,
  open: <FolderOpen size={16} />, log: <FileText size={16} />, delete: <Trash2 size={16} />,
}

interface ContextMenuState { task: Task; x: number; y: number }

export default function TaskTable({ tasks, selected, onSelect, onOpenDetails, onTaskAction, onOpenLog, onOpenFile }: {
  tasks: Task[]
  selected: Set<string>
  onSelect: (ids: Set<string>) => void
  onOpenDetails: (task: Task) => void
  onTaskAction: (task: Task, action: string) => void
  onOpenLog: (task: Task) => void
  onOpenFile: (task: Task) => void
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
    const actions = taskContextActions(task)
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
    else if (action === 'open') onOpenFile(task)
    else if (action === 'log') onOpenLog(task)
    else onTaskAction(task, action)
  }

  if (!tasks.length) return <div className="empty-state"><DownloadCloudIcon /><strong>暂无任务</strong><span>点击“新建”粘贴 m3u8 或普通网页链接</span></div>
  return (
    <div className="table-scroll">
      <table className="task-table">
        <thead><tr><th className="check-col"><input type="checkbox" checked={allSelected} onChange={toggleAll} aria-label="选择全部任务" /></th><th>文件名</th><th>大小</th><th>已下载</th><th>状态</th><th>进度</th><th>速度</th><th>剩余</th><th>分片</th><th>更新时间</th></tr></thead>
        <tbody>{tasks.map(task => {
          const progress = getDisplayedProgress(task)
          return <tr key={task.id} className={selected.has(task.id) ? 'selected' : ''}
            onClick={event => {
              if ((event.target as HTMLElement).closest('input')) return
              if (event.ctrlKey || event.metaKey) toggleOne(task.id)
              else onSelect(new Set([task.id]))
            }}
            onContextMenu={event => openMenu(event, task)}
            onDoubleClick={() => onOpenDetails(task)}>
            <td className="check-col"><input type="checkbox" checked={selected.has(task.id)} onChange={() => toggleOne(task.id)} aria-label={`选择 ${task.title || task.filename || task.id}`} /></td>
            <td className="name-cell"><span title={task.title || task.filename || task.id}>{task.title || task.filename || task.id}</span><small title={task.url}>{task.url}</small></td>
            <td>{fmtBytes(task.total_bytes)}</td><td>{fmtBytes(task.downloaded_bytes)}</td>
            <td><span className={`status status-${task.status}`}>{labels[task.status] || task.status}</span>{task.error_code && <small className="failure-code" title={task.error_message}>{task.error_code}</small>}</td>
            <td><div className="table-progress"><div><i style={{ width: `${Math.max(0, Math.min(100, progress))}%` }} /></div><span>{progress.toFixed(1)}%</span></div></td>
            <td className="speed-cell">{fmtSpeed(task.speed_bytes_per_sec)}</td><td>{fmtEta(task.eta_seconds)}</td>
            <td>{task.total_segments ? `${task.completed_segments}/${task.total_segments}` : '--'}</td><td>{fmtDate(task.updated_at)}</td>
          </tr>
        })}</tbody>
      </table>
      {menu && createPortal(
        <div className="task-context-menu" role="menu" style={{ left: menu.x, top: menu.y }} onPointerDown={event => event.stopPropagation()}>
          {taskContextActions(menu.task).map(action => <button key={action} role="menuitem" className={action === 'delete' ? 'danger' : ''} onClick={() => runMenuAction(action)}>
            {menuIcons[action]}<span>{menuLabels[action]}</span>
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
