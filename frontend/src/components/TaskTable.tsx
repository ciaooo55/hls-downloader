import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AppWindow, Archive, CheckCircle2, File, FileAudio, FileCode2, FileImage, FileText, FileVideo, Film, FolderOpen, Globe2, Info, LoaderCircle, Magnet, MonitorPlay, MoreHorizontal, Pause, Play, PlayCircle, RadioTower, RotateCcw, Trash2, XCircle } from 'lucide-react'
import { getDisplayedProgress } from '../taskState'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import { taskContextActions, type TaskContextAction } from '../taskContextActions'
import { statusLabel } from '../taskPresentation'
import type { Task } from '../types'
import { filePresentation, type FileKind } from '../filePresentation'
import { taskFileUrl } from '../api'

const menuLabels: Record<TaskContextAction, string> = {
  details: '查看详情', start: '开始下载', pause: '暂停', resume: '恢复',
  cancel: '取消任务', retry: '重试', preview: '内置播放', launch: '系统播放', open: '打开文件位置', log: '查看日志', delete: '仅删除任务记录', deleteFiles: '删除任务及文件',
}

const menuIcons: Record<TaskContextAction, React.ReactNode> = {
  details: <Info size={16} />, start: <Play size={16} />, pause: <Pause size={16} />,
  resume: <RotateCcw size={16} />, cancel: <XCircle size={16} />, retry: <RotateCcw size={16} />,
  preview: <MonitorPlay size={16} />, launch: <PlayCircle size={16} />, open: <FolderOpen size={16} />, log: <FileText size={16} />, delete: <Trash2 size={16} />, deleteFiles: <Trash2 size={16} />,
}

interface ContextMenuState { task: Task; taskIds: string[]; actions: TaskContextAction[]; x: number; y: number }

const typeLabels = { hls: 'HLS', dash: 'DASH', http: 'HTTP', torrent: 'BT' }
const typeIcons = {
  hls: <Film size={15} />,
  dash: <RadioTower size={15} />,
  http: <Globe2 size={15} />,
  torrent: <Magnet size={15} />,
}

export default function TaskTable({ tasks, selected, pending, onSelect, onOpenDetails, onTasksAction, onOpenLog, onOpenFile, onLaunchFile, onPreview, onPreviewImage }: {
  tasks: Task[]
  selected: Set<string>
  pending: Set<string>
  onSelect: (ids: Set<string>) => void
  onOpenDetails: (task: Task) => void
  onTasksAction: (tasks: Task[], action: string) => void
  onOpenLog: (task: Task) => void
  onOpenFile: (task: Task) => void
  onLaunchFile: (task: Task) => void
  onPreview: (task: Task) => void
  onPreviewImage: (task: Task) => void
}) {
  const [menu, setMenu] = useState<ContextMenuState | null>(null)
  const selectionAnchor = useRef<string | null>(null)
  const dragStart = useRef<{ index: number; x: number; y: number; base: Set<string> } | null>(null)
  const suppressClick = useRef(false)
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

  const selectRow = (event: React.MouseEvent, task: Task) => {
    if (suppressClick.current) { suppressClick.current = false; return }
    if (event.shiftKey && selectionAnchor.current) {
      const start = tasks.findIndex(value => value.id === selectionAnchor.current)
      const end = tasks.findIndex(value => value.id === task.id)
      if (start >= 0 && end >= 0) {
        const next = new Set(event.ctrlKey || event.metaKey ? selected : [])
        tasks.slice(Math.min(start, end), Math.max(start, end) + 1).forEach(value => next.add(value.id))
        onSelect(next); return
      }
    }
    selectionAnchor.current = task.id
    if (event.ctrlKey || event.metaKey) toggleOne(task.id)
    else onSelect(new Set([task.id]))
  }

  const beginRangeSelection = (event: React.PointerEvent, index: number) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest('input,button,a')) return
    dragStart.current = { index, x: event.clientX, y: event.clientY, base: new Set(event.ctrlKey || event.metaKey ? selected : []) }
    const move = (next: PointerEvent) => {
      const start = dragStart.current
      if (!start || Math.abs(next.clientX - start.x) + Math.abs(next.clientY - start.y) < 6) return
      suppressClick.current = true
      document.body.classList.add('task-range-selecting')
      const row = (document.elementFromPoint(next.clientX, next.clientY) as HTMLElement | null)?.closest<HTMLTableRowElement>('tr[data-task-index]')
      const end = Number(row?.dataset.taskIndex ?? start.index)
      const selection = new Set(start.base)
      tasks.slice(Math.min(start.index, end), Math.max(start.index, end) + 1).forEach(task => selection.add(task.id))
      onSelect(selection)
    }
    const finish = () => {
      dragStart.current = null
      document.body.classList.remove('task-range-selecting')
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish, { once: true })
  }

  const openMenu = (event: React.MouseEvent, task: Task) => {
    event.preventDefault()
    const next = selected.has(task.id) ? new Set(selected) : new Set([task.id])
    onSelect(next)
    const targets = tasks.filter(value => next.has(value.id))
    const hasPending = targets.some(value => pending.has(value.id))
    const actions = hasPending ? (targets.length === 1 ? ['details', 'log'] as TaskContextAction[] : []) : taskContextActions(targets)
    const width = 184
    const height = actions.length * 36 + 12
    setMenu({
      task,
      taskIds: targets.map(value => value.id),
      actions,
      x: Math.max(6, Math.min(event.clientX, window.innerWidth - width - 6)),
      y: Math.max(6, Math.min(event.clientY, window.innerHeight - height - 6)),
    })
  }

  const runMenuAction = (action: TaskContextAction) => {
    if (!menu) return
    const task = menu.task
    const targets = tasks.filter(value => menu.taskIds.includes(value.id))
    setMenu(null)
    if (action === 'details') onOpenDetails(task)
    else if (action === 'preview') onPreview(task)
    else if (action === 'launch') onLaunchFile(task)
    else if (action === 'open') onOpenFile(task)
    else if (action === 'log') onOpenLog(task)
    else onTasksAction(targets, action)
  }

  if (!tasks.length) return <div className="empty-state"><DownloadCloudIcon /><strong>暂无任务</strong><span>点击“新建”添加文件、HLS、DASH、magnet 或种子</span></div>
  return (
    <div className="table-scroll">
      <table className="task-table">
        <thead><tr><th className="check-col"><input type="checkbox" checked={allSelected} onChange={toggleAll} aria-label="选择全部任务" /></th><th>文件名</th><th>状态</th><th>进度</th><th>速度 / 剩余</th><th className="segments-col">分片</th><th className="updated-col">更新时间</th><th className="menu-col" /></tr></thead>
        <tbody>{tasks.map((task, taskIndex) => {
          const progress = getDisplayedProgress(task)
          const visual = filePresentation(task.output_path || task.filename || task.url, task.mime_type)
          const displayName = task.title || task.filename || task.id
          const postProcessing = task.status === 'merging' || task.status === 'remuxing'
          return <tr key={task.id} data-task-index={taskIndex} className={`${selected.has(task.id) ? 'selected ' : ''}${pending.has(task.id) ? 'pending' : ''}`.trim()}
            onClick={event => {
              if ((event.target as HTMLElement).closest('input')) return
              selectRow(event, task)
            }}
            onPointerDown={event => beginRangeSelection(event, taskIndex)}
            onContextMenu={event => openMenu(event, task)}
            onDoubleClick={() => {
              if (task.status === 'done' && visual.kind === 'image' && task.output_is_file) onPreviewImage(task)
              else if ((visual.kind === 'video' || visual.kind === 'audio') && task.available_actions?.includes('preview')) onPreview(task)
              else if (task.status === 'done' && task.output_is_file) onLaunchFile(task)
              else onOpenDetails(task)
            }}>
            <td className="check-col"><input type="checkbox" checked={selected.has(task.id)} onChange={() => toggleOne(task.id)} aria-label={`选择 ${task.title || task.filename || task.id}`} /></td>
            <td className={`name-cell${task.output_is_file ? ' draggable-file' : ''}`} draggable={task.status === 'done' && task.output_is_file} title={task.output_is_file ? '从文件名拖到桌面或资源管理器可复制文件' : undefined} onDragStart={event => {
              if (!task.output_is_file) { event.preventDefault(); return }
              const filename = task.output_path.split(/[\\/]/).pop() || task.filename || task.id
              const url = new URL(taskFileUrl(task.id), window.location.href).href
              event.dataTransfer.effectAllowed = 'copy'
              event.dataTransfer.setData('DownloadURL', `${task.mime_type || 'application/octet-stream'}:${filename}:${url}`)
              event.dataTransfer.setData('text/uri-list', url)
            }}><span title={displayName}><TaskFileIcon kind={visual.kind} extension={visual.extension} /><b>{displayName}</b><i className={`task-type type-${task.task_type}`} title={typeLabels[task.task_type]}>{typeIcons[task.task_type]}</i></span><small title={task.url}>{task.url}</small></td>
            <td><span className={`status status-${task.status}`}>{pending.has(task.id) && <LoaderCircle className="spin" size={12} />}{task.status === 'queued' && task.queue_position ? `排队中 · 第 ${task.queue_position} 位` : statusLabel(task.status)}</span>{task.error_code && <small className="failure-code" title={task.error_message}>{task.error_code}</small>}</td>
            <td>{task.status === 'done'
              ? <span className="completed-progress"><CheckCircle2 size={15} />已完成</span>
              : postProcessing
                ? <div className="phase-progress"><ProgressLine label="下载" value={100} /><ProgressLine label={task.status === 'merging' ? '拼接' : '转封装'} value={task.post_percent || 0} /></div>
                : <><ProgressLine value={progress} /><small className="progress-bytes">{fmtBytes(task.downloaded_bytes)} / {task.total_bytes ? fmtBytes(task.total_bytes) : '--'}</small></>}</td>
            <td><span className="speed-cell">{fmtSpeed(task.speed_bytes_per_sec)}</span><small className="eta-cell">{task.task_type === 'torrent' && task.upload_speed_bytes_per_sec > 0 ? `↑ ${fmtSpeed(task.upload_speed_bytes_per_sec)}` : fmtEta(task.eta_seconds)}</small></td>
            <td className="segments-col" title={task.task_type === 'torrent' ? `Peer ${task.peer_count} · Seed ${task.seed_count}` : `${task.active_workers || task.active_slots || 0}/${task.max_workers || task.concurrency || 0} 个连接`}>{task.task_type === 'torrent' ? `${task.peer_count} Peer` : task.total_segments ? <><span>{task.completed_segments}/{task.total_segments}</span><small>{task.active_workers || task.active_slots || 0} 连接</small></> : '--'}</td><td className="updated-col">{fmtDate(task.updated_at)}</td>
            <td className="menu-col"><button className="row-menu-button" title="任务操作" onClick={event => { event.stopPropagation(); openMenu(event, task) }}><MoreHorizontal size={17} /></button></td>
          </tr>
        })}</tbody>
      </table>
      {menu && createPortal(
        <div className="task-context-menu" role="menu" style={{ left: menu.x, top: menu.y }} onPointerDown={event => event.stopPropagation()}>
          {menu.actions.map(action => <button key={action} role="menuitem" className={action === 'deleteFiles' ? 'danger' : ''} onClick={() => runMenuAction(action)}>
            {menuIcons[action]}<span>{action === 'preview' && menu.task.status !== 'done' ? '边下边播' : action === 'deleteFiles' && menu.taskIds.some(id => tasks.find(task => task.id === id)?.status !== 'done') ? '停止并删除任务及文件' : menuLabels[action]}</span>
          </button>)}
        </div>,
        document.body,
      )}
    </div>
  )
}

function ProgressLine({ value, label }: { value: number; label?: string }) {
  const safe = Math.max(0, Math.min(100, Number(value) || 0))
  return <div className="table-progress">{label && <b>{label}</b>}<div><i style={{ width: `${safe}%` }} /></div><span>{safe.toFixed(1)}%</span></div>
}

const fileKindIcons: Record<FileKind, React.ReactNode> = {
  archive: <Archive size={19} />, executable: <AppWindow size={19} />, video: <FileVideo size={19} />,
  audio: <FileAudio size={19} />, image: <FileImage size={19} />, document: <FileText size={19} />,
  code: <FileCode2 size={19} />, generic: <File size={19} />,
}

function TaskFileIcon({ kind, extension }: { kind: FileKind; extension: string }) {
  return <i className={`file-kind file-kind-${kind}`} title={extension ? `${extension.toUpperCase()} 文件` : '文件'}>{fileKindIcons[kind]}{extension && <em>{extension.slice(0, 4)}</em>}</i>
}

function DownloadCloudIcon() {
  return <div className="empty-glyph">↓</div>
}
