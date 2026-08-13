import { useEffect, useState } from 'react'
import { AppWindow, Archive, AlertCircle, CheckCircle2, Clock3, Download, File, Images, List, PauseCircle, X } from 'lucide-react'
import type { BrowserStatus, Task } from '../types'
import { taskMatchesFilter } from '../taskPresentation'

export const TASK_FILTER_LABELS: Record<TaskFilter, string> = {
  all: '全部任务',
  running: '进行中',
  queued: '排队中',
  paused: '已暂停',
  done: '已完成',
  failed: '失败任务',
  media: '媒体',
  program: '程序',
  archive: '压缩包',
  other: '其他',
}

export function taskFilterLabel(filter: TaskFilter): string {
  return TASK_FILTER_LABELS[filter] || '任务列表'
}

export type TaskFilter = 'all' | 'running' | 'queued' | 'paused' | 'done' | 'failed' | 'media' | 'program' | 'archive' | 'other'

const filters: Array<{ id: TaskFilter; label: string; icon: typeof List }> = [
  { id: 'all', label: '全部任务', icon: List },
  { id: 'running', label: '进行中', icon: Download },
  { id: 'queued', label: '排队中', icon: Clock3 },
  { id: 'paused', label: '已暂停', icon: PauseCircle },
  { id: 'done', label: '已完成', icon: CheckCircle2 },
  { id: 'failed', label: '失败', icon: AlertCircle },
  { id: 'media', label: '媒体', icon: Images },
  { id: 'program', label: '程序', icon: AppWindow },
  { id: 'archive', label: '压缩包', icon: Archive },
  { id: 'other', label: '其他', icon: File },
]

function countFor(tasks: Task[], filter: TaskFilter): number {
  return tasks.filter(task => taskMatchesFilter(task, filter)).length
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
  const activeOutdatedClients = (browserStatus?.clients || []).filter(client => client.active && client.needs_upgrade)
  const browserLabels: Record<string, string> = {
    edge: 'Edge', chrome: 'Chrome', chromium: 'Chromium', brave: 'Brave',
    vivaldi: 'Vivaldi', opera: 'Opera', firefox: 'Firefox', unknown: '未知浏览器',
  }
  const outdatedClientSummary = activeOutdatedClients
    .map(client => `${browserLabels[client.browser] || client.browser} v${client.version || '未知'}`)
    .join('、')
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
            <span>{outdatedClientSummary
              ? `检测到仍在连接的旧插件：${outdatedClientSummary}；建议升级到 v${browserStatus?.recommended_version || '最新'}。`
              : `检测到旧版插件，建议升级到 v${browserStatus?.recommended_version || '最新'}。`}</span>
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
