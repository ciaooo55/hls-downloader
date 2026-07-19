import { useEffect } from 'react'
import { AlertTriangle, FileText, FolderOpen, LoaderCircle, MonitorPlay, Pause, PlayCircle, RotateCcw, Trash2, X, XCircle } from 'lucide-react'
import { getFailureDetails } from '../failureDetails'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import { getDisplayedProgress } from '../taskState'
import { stageLabel, statusLabel } from '../taskPresentation'
import type { Task } from '../types'

export default function TaskDetailsModal({ task, pending, onClose, onLog, onAction, onOpenFile, onLaunchFile, onPreview }: {
  task: Task
  pending: boolean
  onClose: () => void
  onLog: () => void
  onAction: (action: string) => void
  onOpenFile: () => void
  onLaunchFile: () => void
  onPreview: () => void
}) {
  const failure = task.error_message || task.error_code ? getFailureDetails(task) : null
  const actions = task.available_actions || []
  useEffect(() => {
    const close = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [onClose])
  return <div className="modal-overlay" onMouseDown={onClose}><section className="modal task-details" onMouseDown={event => event.stopPropagation()}>
    <header><div><h2>{task.title || task.filename || task.id}</h2><p title={task.url}>{task.url}</p></div><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <div className="detail-grid"><Detail label="状态" value={statusLabel(task.status)} /><Detail label="阶段" value={stageLabel(task.stage)} /><Detail label="总大小" value={fmtBytes(task.total_bytes)} /><Detail label="已下载" value={fmtBytes(task.downloaded_bytes)} /><Detail label="进度" value={`${getDisplayedProgress(task).toFixed(1)}%`} /><Detail label="速度" value={fmtSpeed(task.speed_bytes_per_sec)} /><Detail label="剩余" value={fmtEta(task.eta_seconds)} /><Detail label="分片" value={`${task.completed_segments}/${task.total_segments}`} /><Detail label="活动线程" value={`${task.active_workers}/${task.max_workers}`} /><Detail label="更新时间" value={fmtDate(task.updated_at)} /></div>
    {task.last_log && <div className="detail-message"><b>最近日志</b><code>{task.last_log}</code></div>}
    {failure && <section className="failure-details">
      <h3><AlertTriangle size={17} />{failure.title}</h3>
      {failure.items.length > 0 && <dl>{failure.items.map(item => <div key={item.label}><dt>{item.label}</dt><dd title={item.value}>{item.value}</dd></div>)}</dl>}
      {failure.message && <div className="failure-message"><b>失败原因</b><code>{failure.message}</code></div>}
      {failure.hint && <div className="failure-hint"><b>处理建议</b><span>{failure.hint}</span></div>}
    </section>}
    {task.output_path && <div className="output-path" title={task.output_path}>{task.output_path}</div>}
    <footer className="detail-actions">
      <button className="secondary-button" onClick={onLog}><FileText size={16} />查看日志</button>
      {!pending && actions.includes('pause') && <button className="secondary-button" onClick={() => onAction('pause')}><Pause size={16} />暂停</button>}
      {!pending && actions.includes('resume') && <button className="primary-button" onClick={() => onAction('resume')}><RotateCcw size={16} />恢复</button>}
      {!pending && actions.includes('retry') && <button className="primary-button" onClick={() => onAction('retry')}><RotateCcw size={16} />重试</button>}
      {!pending && actions.includes('cancel') && <button className="secondary-button" onClick={() => onAction('cancel')}><XCircle size={16} />取消</button>}
      {!pending && actions.includes('delete') && <button className="danger-button" onClick={() => onAction('delete')}><Trash2 size={16} />删除记录</button>}
      {actions.includes('open') && <button className="secondary-button" onClick={onOpenFile}><FolderOpen size={16} />所在位置</button>}
      {actions.includes('launch') && <button className="secondary-button" onClick={onLaunchFile}><PlayCircle size={16} />系统播放</button>}
      {actions.includes('preview') && <button className="primary-button" onClick={onPreview}><MonitorPlay size={16} />{task.status === 'done' ? '内置播放' : '边下边播'}</button>}
      {pending && <span className="pending-label"><LoaderCircle className="spin" size={15} />正在处理</span>}
    </footer>
  </section></div>
}

function Detail({ label, value }: { label: string; value: string }) { return <div><span>{label}</span><strong>{value}</strong></div> }
