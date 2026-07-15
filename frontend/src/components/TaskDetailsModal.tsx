import { AlertTriangle, FolderOpen, X } from 'lucide-react'
import { openExplorer } from '../api'
import { getFailureDetails } from '../failureDetails'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import { getDisplayedProgress } from '../taskState'
import type { Task } from '../types'

export default function TaskDetailsModal({ task, onClose, onLog }: { task: Task; onClose: () => void; onLog: () => void }) {
  const failure = task.error_message || task.error_code ? getFailureDetails(task) : null
  return <div className="modal-overlay" onMouseDown={onClose}><section className="modal task-details" onMouseDown={event => event.stopPropagation()}>
    <header><div><h2>{task.title || task.filename || task.id}</h2><p title={task.url}>{task.url}</p></div><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <div className="detail-grid"><Detail label="状态" value={task.status} /><Detail label="阶段" value={task.stage || '--'} /><Detail label="总大小" value={fmtBytes(task.total_bytes)} /><Detail label="已下载" value={fmtBytes(task.downloaded_bytes)} /><Detail label="进度" value={`${getDisplayedProgress(task).toFixed(1)}%`} /><Detail label="速度" value={fmtSpeed(task.speed_bytes_per_sec)} /><Detail label="剩余" value={fmtEta(task.eta_seconds)} /><Detail label="分片" value={`${task.completed_segments}/${task.total_segments}`} /><Detail label="活动线程" value={`${task.active_workers}/${task.max_workers}`} /><Detail label="更新时间" value={fmtDate(task.updated_at)} /></div>
    {task.last_log && <div className="detail-message"><b>最近日志</b><code>{task.last_log}</code></div>}
    {failure && <section className="failure-details">
      <h3><AlertTriangle size={17} />{failure.title}</h3>
      {failure.items.length > 0 && <dl>{failure.items.map(item => <div key={item.label}><dt>{item.label}</dt><dd title={item.value}>{item.value}</dd></div>)}</dl>}
      {failure.message && <div className="failure-message"><b>失败原因</b><code>{failure.message}</code></div>}
      {failure.hint && <div className="failure-hint"><b>处理建议</b><span>{failure.hint}</span></div>}
    </section>}
    {task.output_path && <div className="output-path" title={task.output_path}>{task.output_path}</div>}
    <footer><button className="secondary-button" onClick={onLog}>查看日志</button>{task.output_path && <button className="primary-button" onClick={() => openExplorer(task.output_path)}><FolderOpen size={16} />打开所在位置</button>}</footer>
  </section></div>
}

function Detail({ label, value }: { label: string; value: string }) { return <div><span>{label}</span><strong>{value}</strong></div> }
