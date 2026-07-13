import { CheckCircle2, CirclePause, CircleX, Download, List, Radio, XCircle } from 'lucide-react'
import type { Task, UserscriptStatus } from '../types'

export type TaskFilter = 'all' | 'running' | 'paused' | 'done' | 'failed' | 'canceled'

const filters: Array<{ id: TaskFilter; label: string; icon: typeof List }> = [
  { id: 'all', label: '全部任务', icon: List },
  { id: 'running', label: '下载中', icon: Download },
  { id: 'paused', label: '已暂停', icon: CirclePause },
  { id: 'done', label: '已完成', icon: CheckCircle2 },
  { id: 'failed', label: '失败', icon: CircleX },
  { id: 'canceled', label: '已取消', icon: XCircle },
]

function countFor(tasks: Task[], filter: TaskFilter): number {
  if (filter === 'all') return tasks.length
  if (filter === 'running') return tasks.filter(task => ['queued', 'downloading', 'downloading_m3u8', 'parsing', 'downloading_segments', 'pausing', 'merging', 'remuxing'].includes(task.status)).length
  return tasks.filter(task => task.status === filter).length
}

export default function Sidebar({ tasks, active, onChange, userscript }: { tasks: Task[]; active: TaskFilter; onChange: (filter: TaskFilter) => void; userscript: UserscriptStatus | null }) {
  return (
    <aside className="sidebar">
      <nav>
        {filters.map(item => {
          const Icon = item.icon
          return <button key={item.id} className={`sidebar-item${active === item.id ? ' active' : ''}`} onClick={() => onChange(item.id)}><Icon size={16} /><span>{item.label}</span><b>{countFor(tasks, item.id)}</b></button>
        })}
      </nav>
      <div className="sidebar-script">
        <span className="sidebar-caption">油猴脚本</span>
        <div className={`script-state ${userscript?.detected ? 'online' : userscript?.seen_before ? 'seen' : ''}`}><Radio size={15} /><span>{userscript?.detected ? '已检测运行' : userscript?.seen_before ? '此前运行过' : '尚未检测'}</span></div>
        {userscript?.version && <small>版本 {userscript.version}</small>}
      </div>
    </aside>
  )
}
