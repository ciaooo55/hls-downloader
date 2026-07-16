import {
  ClipboardPaste, FileText, FolderOpen, Layers3, Moon, Pause, Play,
  CircleArrowUp, Plus, RefreshCw, RotateCcw, Settings, Sun, Trash2, Users, XCircle,
} from 'lucide-react'
import type { CommandState } from '../taskCommands'
import type { Theme } from '../theme'

interface Props {
  commands: CommandState
  theme: Theme
  version: string
  onNew: () => void
  onPaste: () => void
  onBatch: () => void
  onAction: (action: string) => void
  onOpen: () => void
  onLog: () => void
  onUserscript: () => void
  onRefresh: () => void
  onUpdate: () => void
  onSettings: () => void
  onToggleTheme: () => void
}

function ToolButton({ title, disabled, onClick, children, primary = false }: {
  title: string; disabled?: boolean; onClick: () => void; children: React.ReactNode; primary?: boolean
}) {
  return <button className={`tool-button${primary ? ' primary' : ''}`} title={title} aria-label={title} disabled={disabled} onClick={onClick}>{children}</button>
}

export default function DesktopToolbar(props: Props) {
  const c = props.commands
  return (
    <header className="desktop-toolbar">
      <div className="brand-block"><img className="app-mark" src="/ui/app-icon.png" alt="" /><div><strong>HLS Downloader</strong><span>桌面下载管理器{props.version ? ` · v${props.version}` : ''}</span></div></div>
      <div className="tool-group">
        <ToolButton title="新建并识别链接" onClick={props.onNew} primary><Plus size={18} /><span>新建</span></ToolButton>
        <ToolButton title="粘贴并识别" onClick={props.onPaste}><ClipboardPaste size={18} /></ToolButton>
        <ToolButton title="批量添加" onClick={props.onBatch}><Layers3 size={18} /></ToolButton>
      </div>
      <div className="toolbar-divider" />
      <div className="tool-group">
        <ToolButton title="开始" disabled={!c.start} onClick={() => props.onAction('start')}><Play size={18} /></ToolButton>
        <ToolButton title="暂停" disabled={!c.pause} onClick={() => props.onAction('pause')}><Pause size={18} /></ToolButton>
        <ToolButton title="恢复" disabled={!c.resume} onClick={() => props.onAction('resume')}><RotateCcw size={18} /></ToolButton>
        <ToolButton title="取消" disabled={!c.cancel} onClick={() => props.onAction('cancel')}><XCircle size={18} /></ToolButton>
        <ToolButton title="重试" disabled={!c.retry} onClick={() => props.onAction('retry')}><RefreshCw size={18} /></ToolButton>
        <ToolButton title="打开文件所在位置" disabled={!c.open} onClick={props.onOpen}><FolderOpen size={18} /></ToolButton>
        <ToolButton title="查看日志" disabled={!c.log} onClick={props.onLog}><FileText size={18} /></ToolButton>
        <ToolButton title="删除任务" disabled={!c.delete} onClick={() => props.onAction('delete')}><Trash2 size={18} /></ToolButton>
      </div>
      <div className="toolbar-spacer" />
      <div className="tool-group">
        <ToolButton title="油猴脚本工具" onClick={props.onUserscript}><Users size={18} /></ToolButton>
        <ToolButton title="刷新任务" onClick={props.onRefresh}><RefreshCw size={18} /></ToolButton>
        <button className="tool-button update-button" title="检查软件更新" aria-label="检查软件更新" onClick={props.onUpdate}><CircleArrowUp size={18} /><span>更新</span></button>
        <ToolButton title={props.theme === 'dark' ? '切换浅色主题' : '切换深色主题'} onClick={props.onToggleTheme}>{props.theme === 'dark' ? <Sun size={18} /> : <Moon size={18} />}</ToolButton>
        <ToolButton title="设置" onClick={props.onSettings}><Settings size={18} /></ToolButton>
      </div>
    </header>
  )
}
