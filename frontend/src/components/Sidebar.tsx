import { useEffect, useState } from 'react'
import { AppWindow, Archive, AlertCircle, CheckCircle2, Download, File, Images, List, X } from 'lucide-react'
import type { BrowserStatus, Task } from '../types'
import { downloadCategory } from '../downloadCategory'

export type TaskFilter = 'all' | 'running' | 'done' | 'failed' | 'media' | 'program' | 'archive' | 'other'

const filters: Array<{ id: TaskFilter; label: string; icon: typeof List }> = [
  { id: 'all', label: '全部任务', icon: List },
  { id: 'running', label: '进行中', icon: Download },
  { id: 'done', label: '已完成', icon: CheckCircle2 },
  { id: 'failed', label: '失败', icon: AlertCircle },
  { id: 'media', label: '媒体', icon: Images },
  { id: 'program', label: '程序', icon: AppWindow },
  { id: 'archive', label: '压缩包', icon: Archive },
  { id: 'other', label: '其他', icon: File },
]

function countFor(tasks: Task[], filter: TaskFilter): number {
  if (filter === 'all') return tasks.length
  if (filter === 'running') return tasks.filter(task => ['queued', 'fetching_metadata', 'checking', 'downloading', 'downloading_m3u8', 'parsing', 'downloading_segments', 'pausing', 'merging', 'remuxing'].includes(task.status)).length
  if (filter === 'failed') return tasks.filter(task => task.status === 'failed' || task.status === 'unsupported').length
  if (['media', 'program', 'archive', 'other'].includes(filter)) return tasks.filter(task => downloadCategory(task.output_path || task.filename || task.url, task.mime_type, task.task_type) === filter).length
  return tasks.filter(task => task.status === filter).length
}

export default function Sidebar({ tasks, active, onChange, browserStatus, appVersion = '', onOpenExtensionHelp }: {
  tasks: Task[]
  active: TaskFilter
  onChange: (filter: TaskFilter) => void
  browserStatus: BrowserStatus | null
  appVersion?: string
  onOpenExtensionHelp?: () => void
}) {
  const serviceOnline = Boolean(appVersion)
  const extensionOnline = Boolean(browserStatus?.detected)
  const extensionLost = !extensionOnline && Boolean(browserStatus?.seen_before)
  const extensionNeedsUpgrade = Boolean(extensionOnline && browserStatus?.needs_upgrade)
  const [bubbleDismissed, setBubbleDismissed] = useState(false)
  useEffect(() => { setBubbleDismissed(false) }, [extensionOnline, extensionNeedsUpgrade])
  return (
    <aside className="sidebar">
      <nav>
        {filters.map((item, index) => {
          const Icon = item.icon
          return <div key={item.id}>{index === 4 && <span className="sidebar-group-label">分类</span>}<button title={`${item.label} · ${countFor(tasks, item.id)}`} aria-label={item.label} className={`sidebar-item${active === item.id ? ' active' : ''}`} onClick={() => onChange(item.id)}><Icon size={18} /><span>{item.label}</span><b>{countFor(tasks, item.id)}</b></button></div>
        })}
      </nav>
      <div className="sidebar-connection">
        <span className="sidebar-caption">连接</span>
        <div className={`connection-row ${serviceOnline ? 'online' : 'offline'}`}><i className="connection-dot" /><span>本地服务</span><b>{serviceOnline ? '正常' : '离线'}</b></div>
        <div className={`connection-row ${extensionOnline ? 'online' : 'offline'}${extensionNeedsUpgrade ? ' warning' : ''}`} title={browserStatus?.message || ''}><i className="connection-dot" /><span>浏览器插件</span><b>{extensionNeedsUpgrade ? '需升级' : extensionOnline ? '已连接' : extensionLost ? '已断开' : '未连接'}</b></div>
        <small>{appVersion ? `v${appVersion}` : ''}{browserStatus?.version ? ` · 插件 v${browserStatus.version}` : ''}</small>
        {extensionNeedsUpgrade && !bubbleDismissed && (
          <div className="connection-bubble warning" role="status">
            <button className="connection-bubble-close" aria-label="关闭提示" onClick={() => setBubbleDismissed(true)}><X size={13} /></button>
            <b>插件版本需要同步</b>
            <span>当前插件 v{browserStatus?.version || '未知'}，建议使用本版本发布包中的 v{browserStatus?.recommended_version || '最新'} 插件；旧插件仍可继续下载。</span>
            <button className="secondary-button" onClick={onOpenExtensionHelp}>查看插件版本</button>
          </div>
        )}
        {extensionLost && !bubbleDismissed && (
          <div className="connection-bubble" role="status">
            <button className="connection-bubble-close" aria-label="关闭提示" onClick={() => setBubbleDismissed(true)}><X size={13} /></button>
            <b>插件连接已断开</b>
            <span>浏览器可能重启过，或插件被更新/停用；网页嗅探和接管暂不可用。</span>
            <button className="secondary-button" onClick={onOpenExtensionHelp}>查看排查步骤</button>
          </div>
        )}
      </div>
    </aside>
  )
}
