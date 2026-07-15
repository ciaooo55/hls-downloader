import { getDisplayedProgress } from '../taskState'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import type { Task } from '../types'

const labels: Record<string, string> = {
  queued: '排队中', downloading: '下载中', downloading_m3u8: '获取清单', parsing: '解析中',
  downloading_segments: '下载分片', pausing: '暂停中', paused: '已暂停', merging: '合并中',
  remuxing: '转封装', done: '已完成', failed: '失败', canceled: '已取消', unsupported: '不支持',
}

export default function TaskTable({ tasks, selected, onSelect, onOpenDetails }: {
  tasks: Task[]; selected: Set<string>; onSelect: (ids: Set<string>) => void; onOpenDetails: (task: Task) => void
}) {
  const allSelected = tasks.length > 0 && tasks.every(task => selected.has(task.id))
  const toggleAll = () => onSelect(allSelected ? new Set() : new Set(tasks.map(task => task.id)))
  const toggleOne = (id: string) => {
    const next = new Set(selected)
    if (next.has(id)) next.delete(id); else next.add(id)
    onSelect(next)
  }

  if (!tasks.length) return <div className="empty-state"><DownloadCloudIcon /><strong>暂无任务</strong><span>点击“新建”粘贴 m3u8 或普通网页链接</span></div>
  return (
    <div className="table-scroll">
      <table className="task-table">
        <thead><tr><th className="check-col"><input type="checkbox" checked={allSelected} onChange={toggleAll} aria-label="选择全部任务" /></th><th>文件名</th><th>大小</th><th>已下载</th><th>状态</th><th>进度</th><th>速度</th><th>剩余</th><th>分片</th><th>更新时间</th></tr></thead>
        <tbody>{tasks.map(task => {
          const progress = getDisplayedProgress(task)
          return <tr key={task.id} className={selected.has(task.id) ? 'selected' : ''} onDoubleClick={() => onOpenDetails(task)}>
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
    </div>
  )
}

function DownloadCloudIcon() {
  return <div className="empty-glyph">↓</div>
}
