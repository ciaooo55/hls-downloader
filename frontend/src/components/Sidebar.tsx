import { CheckCircle2, CirclePause, CircleX, Download, Film, Globe2, List, Magnet, Radio, RadioTower, XCircle } from 'lucide-react'
import type { BrowserStatus, Task, UserscriptStatus } from '../types'

export type TaskFilter = 'all' | 'running' | 'paused' | 'done' | 'failed' | 'canceled' | 'hls' | 'dash' | 'http' | 'torrent'

const filters: Array<{ id: TaskFilter; label: string; icon: typeof List }> = [
  { id: 'all', label: '全部任务', icon: List },
  { id: 'running', label: '下载中', icon: Download },
  { id: 'paused', label: '已暂停', icon: CirclePause },
  { id: 'done', label: '已完成', icon: CheckCircle2 },
  { id: 'failed', label: '失败', icon: CircleX },
  { id: 'canceled', label: '已取消', icon: XCircle },
  { id: 'hls', label: 'HLS 视频', icon: Film },
  { id: 'dash', label: 'DASH 视频', icon: RadioTower },
  { id: 'http', label: '普通文件', icon: Globe2 },
  { id: 'torrent', label: 'BT 下载', icon: Magnet },
]

function countFor(tasks: Task[], filter: TaskFilter): number {
  if (filter === 'all') return tasks.length
  if (filter === 'running') return tasks.filter(task => ['queued', 'fetching_metadata', 'checking', 'downloading', 'downloading_m3u8', 'parsing', 'downloading_segments', 'pausing', 'merging', 'remuxing'].includes(task.status)).length
  if (['hls', 'dash', 'http', 'torrent'].includes(filter)) return tasks.filter(task => task.task_type === filter).length
  return tasks.filter(task => task.status === filter).length
}

export default function Sidebar({ tasks, active, onChange, userscript, browserStatus }: { tasks: Task[]; active: TaskFilter; onChange: (filter: TaskFilter) => void; userscript: UserscriptStatus | null; browserStatus: BrowserStatus | null }) {
  return (
    <aside className="sidebar">
      <nav>
        {filters.map(item => {
          const Icon = item.icon
          return <button key={item.id} className={`sidebar-item${active === item.id ? ' active' : ''}`} onClick={() => onChange(item.id)}><Icon size={16} /><span>{item.label}</span><b>{countFor(tasks, item.id)}</b></button>
        })}
      </nav>
      <div className="sidebar-script">
        <span className="sidebar-caption">{browserStatus?.detected ? '浏览器扩展' : '浏览器脚本'}</span>
        <div className={`script-state ${browserStatus?.detected || userscript?.detected ? 'online' : browserStatus?.seen_before || userscript?.seen_before ? 'seen' : ''}`}><Radio size={15} /><span>{browserStatus?.detected ? '正式扩展已连接' : userscript?.detected ? '后备脚本已连接' : browserStatus?.seen_before || userscript?.seen_before ? '此前运行过' : '尚未检测'}</span></div>
        {(browserStatus?.version || userscript?.version) && <small>版本 {browserStatus?.version || userscript?.version}</small>}
      </div>
    </aside>
  )
}
