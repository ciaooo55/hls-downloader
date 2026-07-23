import { AppWindow, Archive, CheckCircle2, Download, File, Images, List, Radio } from 'lucide-react'
import type { BrowserStatus, Task } from '../types'
import { downloadCategory } from '../downloadCategory'

export type TaskFilter = 'all' | 'running' | 'done' | 'media' | 'program' | 'archive' | 'other'

const filters: Array<{ id: TaskFilter; label: string; icon: typeof List }> = [
  { id: 'all', label: '全部任务', icon: List },
  { id: 'running', label: '进行中', icon: Download },
  { id: 'done', label: '已完成', icon: CheckCircle2 },
  { id: 'media', label: '媒体', icon: Images },
  { id: 'program', label: '程序', icon: AppWindow },
  { id: 'archive', label: '压缩包', icon: Archive },
  { id: 'other', label: '其他', icon: File },
]

function countFor(tasks: Task[], filter: TaskFilter): number {
  if (filter === 'all') return tasks.length
  if (filter === 'running') return tasks.filter(task => ['queued', 'fetching_metadata', 'checking', 'downloading', 'downloading_m3u8', 'parsing', 'downloading_segments', 'pausing', 'merging', 'remuxing'].includes(task.status)).length
  if (['media', 'program', 'archive', 'other'].includes(filter)) return tasks.filter(task => downloadCategory(task.output_path || task.filename || task.url, task.mime_type, task.task_type) === filter).length
  return tasks.filter(task => task.status === filter).length
}

export default function Sidebar({ tasks, active, onChange, browserStatus }: { tasks: Task[]; active: TaskFilter; onChange: (filter: TaskFilter) => void; browserStatus: BrowserStatus | null }) {
  return (
    <aside className="sidebar">
      <div className="sidebar-brand" title="HLS Downloader"><img src="./app-icon.png" alt="" /></div>
      <nav>
        {filters.map((item, index) => {
          const Icon = item.icon
          return <div key={item.id}>{index === 3 && <span className="sidebar-group-label">分类</span>}<button title={`${item.label} · ${countFor(tasks, item.id)}`} aria-label={item.label} className={`sidebar-item${active === item.id ? ' active' : ''}`} onClick={() => onChange(item.id)}><Icon size={18} /><span>{item.label}</span><b>{countFor(tasks, item.id)}</b></button></div>
        })}
      </nav>
      <div className="sidebar-script">
        <span className="sidebar-caption">浏览器接管</span>
        <div className={`script-state ${browserStatus?.detected ? 'online' : browserStatus?.seen_before ? 'seen' : ''}`} title={browserStatus?.message || ''}><Radio size={15} /><span>{browserStatus?.detected ? '正式插件已连接' : browserStatus?.seen_before ? '插件连接已断开' : '插件未安装或未连接'}</span></div>
        {browserStatus?.version && <small>版本 {browserStatus.version}</small>}
      </div>
    </aside>
  )
}
