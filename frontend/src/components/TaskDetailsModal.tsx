import { useEffect, useState } from 'react'
import { AlertTriangle, FileText, FolderOpen, LoaderCircle, MonitorPlay, Pause, PlayCircle, RotateCcw, Trash2, X, XCircle } from 'lucide-react'
import { getFailureDetails } from '../failureDetails'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import { getDisplayedProgress } from '../taskState'
import { stageLabel, statusLabel } from '../taskPresentation'
import type { Task } from '../types'
import { fetchTorrentFiles, selectTorrentFiles } from '../api'
import { Button, Dialog, DialogOverlay } from './ui'

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
  const [torrentFiles, setTorrentFiles] = useState<Array<{ index: number; path: string; size: number }>>([])
  const [selectedFiles, setSelectedFiles] = useState<number[]>([])
  const [selectionBusy, setSelectionBusy] = useState(false)
  useEffect(() => {
    const close = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [onClose])
  useEffect(() => {
    if (task.task_type !== 'torrent') return
    fetchTorrentFiles(task.id).then(result => {
      setTorrentFiles(result.files || [])
      setSelectedFiles(result.selected || [])
    }).catch(() => {})
  }, [task.id, task.task_type, task.status])
  return <DialogOverlay onClose={onClose}><Dialog className="task-details" label="任务详情">
    <header><div><h2>{task.title || task.filename || task.id}</h2><p title={task.url}>{task.url}</p></div><button className="modal-close-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <div className="task-details-body">
      <div className="detail-grid"><Detail label="类型" value={task.task_type.toUpperCase()} /><Detail label="状态" value={statusLabel(task.status)} /><Detail label="阶段" value={stageLabel(task.stage)} /><Detail label="进度" value={`${getDisplayedProgress(task).toFixed(1)}%`} /><Detail label="总大小" value={fmtBytes(task.total_bytes)} /><Detail label="已下载" value={fmtBytes(task.downloaded_bytes)} /><Detail label="下载速度" value={fmtSpeed(task.speed_bytes_per_sec)} /><Detail label="剩余时间" value={fmtEta(task.eta_seconds)} /><Detail label={task.task_type === 'torrent' ? 'Piece' : '分片'} value={`${task.completed_segments}/${task.total_segments}`} /><Detail label={task.task_type === 'torrent' ? 'Peer / Seed' : '活动线程'} value={task.task_type === 'torrent' ? `${task.peer_count} / ${task.seed_count}` : `${task.active_workers}/${task.max_workers}`} /><Detail label="上传速度" value={task.task_type === 'torrent' ? fmtSpeed(task.upload_speed_bytes_per_sec) : '--'} /><Detail label="更新时间" value={fmtDate(task.updated_at)} /></div>
      {task.last_log && <div className="detail-message"><b>最近日志</b><code>{task.last_log}</code></div>}
      {task.expected_checksum && <section className={`checksum-details ${task.checksum_verified === false ? 'failed' : task.checksum_verified ? 'verified' : ''}`}><b>文件校验</b><dl><div><dt>期望</dt><dd>{task.expected_checksum}</dd></div><div><dt>结果</dt><dd>{task.checksum_verified === true ? '已通过' : task.checksum_verified === false ? '不匹配或未能校验' : '等待下载完成'}</dd></div>{task.checksum_actual && <div><dt>实际</dt><dd>{task.checksum_actual}</dd></div>}</dl></section>}
      {failure && <section className="failure-details">
      <h3><AlertTriangle size={17} />{failure.title}</h3>
      {failure.items.length > 0 && <dl>{failure.items.map(item => <div key={item.label}><dt>{item.label}</dt><dd title={item.value}>{item.value}</dd></div>)}</dl>}
      {failure.message && <div className="failure-message"><b>失败原因</b><code>{failure.message}</code></div>}
      {failure.hint && <div className="failure-hint"><b>处理建议</b><span>{failure.hint}</span></div>}
      {failure.steps && failure.steps.length > 0 && (
        <div className="failure-steps">
          <b>建议步骤</b>
          <ol>{failure.steps.map((step: string) => <li key={step}>{step}</li>)}</ol>
        </div>
      )}
      </section>}
      {task.task_type === 'torrent' && torrentFiles.length > 0 && <section className="torrent-files">
      <div className="torrent-files-head"><h3>BT 文件选择</h3><span>{selectedFiles.length}/{torrentFiles.length}</span></div>
      <div className="torrent-file-list">{torrentFiles.map(file => <label key={file.index}><input type="checkbox" checked={selectedFiles.includes(file.index)} disabled={task.status === 'downloading' || task.status === 'done'} onChange={event => setSelectedFiles(current => event.target.checked ? [...current, file.index] : current.filter(index => index !== file.index))} /><span title={file.path}>{file.path}</span><b>{fmtBytes(file.size)}</b></label>)}</div>
      {task.status !== 'downloading' && task.status !== 'done' && <button className="secondary-button" disabled={selectionBusy || !selectedFiles.length} onClick={async () => { setSelectionBusy(true); try { await selectTorrentFiles(task.id, selectedFiles) } finally { setSelectionBusy(false) } }}>保存文件选择</button>}
      </section>}
      {task.output_path && <div className="output-path" title={task.output_path}>{task.output_path}</div>}
    </div>
    <footer className="detail-actions">
      <button className="secondary-button" onClick={onLog}><FileText size={16} />查看日志</button>
      {!pending && actions.includes('pause') && <button className="secondary-button" onClick={() => onAction('pause')}><Pause size={16} />暂停</button>}
      {!pending && actions.includes('resume') && <button className="primary-button" onClick={() => onAction('resume')}><RotateCcw size={16} />恢复</button>}
      {!pending && actions.includes('retry') && <button className="primary-button" onClick={() => onAction('retry')}><RotateCcw size={16} />重试</button>}
      {!pending && actions.includes('cancel') && <button className="secondary-button" onClick={() => onAction('cancel')}><XCircle size={16} />取消</button>}
      {!pending && actions.includes('delete') && <button className="danger-button" onClick={() => onAction('delete')}><Trash2 size={16} />删除记录</button>}
      {!pending && actions.includes('delete_files') && <button className="danger-button" onClick={() => onAction('deleteFiles')}><Trash2 size={16} />{task.status === 'done' ? '删除任务及文件' : '停止并删除'}</button>}
      {actions.includes('open') && <button className="secondary-button" onClick={onOpenFile}><FolderOpen size={16} />所在位置</button>}
      {actions.includes('launch') && <button className="secondary-button" onClick={onLaunchFile}><PlayCircle size={16} />系统播放</button>}
      {actions.includes('preview') && <button className="primary-button" onClick={onPreview}><MonitorPlay size={16} />{task.status === 'done' ? '内置播放' : '边下边播'}</button>}
      {pending && <span className="pending-label"><LoaderCircle className="spin" size={15} />正在处理</span>}
    </footer>
  </Dialog></DialogOverlay>
}

function Detail({ label, value }: { label: string; value: string }) { return <div><span>{label}</span><strong>{value}</strong></div> }
