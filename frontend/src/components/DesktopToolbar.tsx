import {
  ClipboardPaste, FileText, FolderOpen, Globe2, Layers3, Moon, MoreHorizontal, Pause, Play,
  CircleArrowUp, FileDown, Plus, RefreshCw, RotateCcw, ScreenShare, Search, Settings, Sun, Trash2, Tv, Users, X, XCircle,
} from 'lucide-react'
import type { CommandState } from '../taskCommands'
import type { Theme } from '../theme'
import { Button } from './ui/button'
import { cn } from '../lib/cn'

interface Props {
  commands: CommandState
  theme: Theme
  version: string
  query: string
  onQueryChange: (value: string) => void
  onNew: () => void
  onPaste: () => void
  onBatch: () => void
  onHarvest: () => void
  onExportUrls: () => void
  onAction: (action: string) => void
  onPauseAll: () => void
  onStartAll: () => void
  onOpen: () => void
  onLog: () => void
  onBrowserExtension: () => void
  onPushMedia: () => void
  pushLocalMediaBusy: boolean
  onCastMedia: () => void
  castLocalMediaBusy: boolean
  onRefresh: () => void
  onUpdate: () => void
  onSettings: () => void
  onToggleTheme: () => void
}

function ToolButton({
  title,
  disabled,
  onClick,
  children,
  primary = false,
}: {
  title: string
  disabled?: boolean
  onClick: () => void
  children: React.ReactNode
  primary?: boolean
}) {
  return (
    <Button
      variant={primary ? 'primaryTool' : 'tool'}
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className={cn('tool-button', primary && 'primary')}
    >
      {children}
    </Button>
  )
}

export default function DesktopToolbar(props: Props) {
  const c = props.commands
  const closeOverflow = (event: React.MouseEvent<HTMLButtonElement>) => {
    event.currentTarget.closest('details')?.removeAttribute('open')
  }
  return (
    <header className="desktop-toolbar">
      <div className="brand-block">
        <img className="app-mark" src="./app-icon.png" alt="" />
        <div>
          <strong>HLS Downloader</strong>
          <span>本地高性能下载管理器</span>
        </div>
      </div>
      <div className="tool-group">
        <ToolButton title="新建并识别链接" onClick={props.onNew} primary>
          <Plus size={18} />
          <span>新建</span>
        </ToolButton>
        <ToolButton title="粘贴并识别" onClick={props.onPaste}>
          <ClipboardPaste size={17} />
          <span className="tool-label">粘贴</span>
        </ToolButton>
        <ToolButton title="批量添加" onClick={props.onBatch}>
          <Layers3 size={17} />
          <span className="tool-label">批量</span>
        </ToolButton>
      </div>
      <div className="toolbar-divider" />
      <div className="tool-group">
        <ToolButton title="开始" disabled={!c.start} onClick={() => props.onAction('start')}>
          <Play size={17} />
          <span className="tool-label">开始</span>
        </ToolButton>
        <ToolButton title={c.pauseLabel} disabled={!c.pause} onClick={() => props.onAction('pause')}>
          <Pause size={17} />
          <span className="tool-label">{c.pauseLabel === '停止录制' ? '停止' : '暂停'}</span>
        </ToolButton>
        <ToolButton title="全部开始（排队/暂停）" onClick={props.onStartAll}>
          <Play size={17} />
          <span className="tool-label">全开</span>
        </ToolButton>
        <ToolButton title="全部暂停" onClick={props.onPauseAll}>
          <Pause size={17} />
          <span className="tool-label">全停</span>
        </ToolButton>
        <ToolButton title={c.resumeLabel} disabled={!c.resume} onClick={() => props.onAction('resume')}>
          <RotateCcw size={18} />
        </ToolButton>
        <ToolButton title="取消" disabled={!c.cancel} onClick={() => props.onAction('cancel')}>
          <XCircle size={18} />
        </ToolButton>
        <ToolButton title="重试" disabled={!c.retry} onClick={() => props.onAction('retry')}>
          <RefreshCw size={18} />
        </ToolButton>
        <ToolButton title="打开文件所在位置" disabled={!c.open} onClick={props.onOpen}>
          <FolderOpen size={18} />
        </ToolButton>
        <ToolButton title="查看日志" disabled={!c.log} onClick={props.onLog}>
          <FileText size={18} />
        </ToolButton>
        <ToolButton title="删除任务" disabled={!c.delete} onClick={() => props.onAction('delete')}>
          <Trash2 size={18} />
        </ToolButton>
      </div>
      <div className="toolbar-spacer" />
      <label className="toolbar-search">
        <Search size={14} />
        <input
          value={props.query}
          onChange={event => props.onQueryChange(event.target.value)}
          placeholder="搜索下载"
          aria-label="搜索任务、链接或错误码"
        />
        {props.query && (
          <button type="button" title="清除搜索" aria-label="清除搜索" onClick={() => props.onQueryChange('')}>
            <X size={13} />
          </button>
        )}
      </label>
      <div className="tool-group">
        <ToolButton title="浏览器插件" onClick={props.onBrowserExtension}>
          <Users size={18} />
        </ToolButton>
        <ToolButton title="选择媒体链接或本机文件，再投屏到 DLNA / Chromecast 设备" disabled={props.castLocalMediaBusy} onClick={props.onCastMedia}>
          <ScreenShare size={18} />
          <span className="tool-label">投屏</span>
        </ToolButton>
        <ToolButton title="选择媒体链接或本机文件，再推送到 TVBox" disabled={props.pushLocalMediaBusy} onClick={props.onPushMedia}>
          <Tv size={18} />
          <span className="tool-label">TVBox</span>
        </ToolButton>
        <ToolButton title="刷新任务" onClick={props.onRefresh}>
          <RefreshCw size={18} />
        </ToolButton>
        <Button
          variant="tool"
          className="tool-button update-button"
          title={`检查软件更新${props.version ? ` · 当前 v${props.version}` : ''}`}
          aria-label="检查软件更新"
          onClick={props.onUpdate}
        >
          <CircleArrowUp size={18} />
        </Button>
        <ToolButton title={props.theme === 'dark' ? '切换浅色主题' : '切换深色主题'} onClick={props.onToggleTheme}>
          {props.theme === 'dark' ? <Sun size={18} /> : <Moon size={18} />}
        </ToolButton>
        <ToolButton title="设置" onClick={props.onSettings}>
          <Settings size={18} />
        </ToolButton>
      </div>
      <details className="toolbar-overflow">
        <summary title="更多操作" aria-label="更多操作"><MoreHorizontal size={18} /></summary>
        <div className="toolbar-overflow-menu" role="menu" aria-label="更多操作">
          <button type="button" role="menuitem" onClick={event => { closeOverflow(event); props.onBatch() }}><Layers3 size={16} />批量添加</button>
          <button type="button" role="menuitem" onClick={event => { closeOverflow(event); props.onHarvest() }}><Globe2 size={16} />网页抓取</button>
          <button type="button" role="menuitem" onClick={event => { closeOverflow(event); props.onExportUrls() }}><FileDown size={16} />导出链接</button>
          <button type="button" role="menuitem" onClick={event => { closeOverflow(event); props.onStartAll() }}><Play size={16} />全部开始</button>
          <button type="button" role="menuitem" onClick={event => { closeOverflow(event); props.onPauseAll() }}><Pause size={16} />全部暂停</button>
          <button type="button" disabled={!c.resume} role="menuitem" onClick={event => { closeOverflow(event); props.onAction('resume') }}><RotateCcw size={16} />恢复任务</button>
          <button type="button" disabled={!c.cancel} role="menuitem" onClick={event => { closeOverflow(event); props.onAction('cancel') }}><XCircle size={16} />取消任务</button>
          <button type="button" disabled={!c.open} role="menuitem" onClick={event => { closeOverflow(event); props.onOpen() }}><FolderOpen size={16} />打开文件位置</button>
          <button type="button" disabled={!c.log} role="menuitem" onClick={event => { closeOverflow(event); props.onLog() }}><FileText size={16} />查看日志</button>
          <button type="button" disabled={!c.delete} role="menuitem" className="danger" onClick={event => { closeOverflow(event); props.onAction('delete') }}><Trash2 size={16} />删除任务</button>
          <span className="toolbar-overflow-separator" />
          <button type="button" role="menuitem" onClick={event => { closeOverflow(event); props.onBrowserExtension() }}><Users size={16} />浏览器插件</button>
          <button type="button" disabled={props.castLocalMediaBusy} role="menuitem" onClick={event => { closeOverflow(event); props.onCastMedia() }}><ScreenShare size={16} />投屏媒体</button>
          <button type="button" disabled={props.pushLocalMediaBusy} role="menuitem" onClick={event => { closeOverflow(event); props.onPushMedia() }}><Tv size={16} />TVBox 推送媒体</button>
          <button type="button" role="menuitem" onClick={event => { closeOverflow(event); props.onUpdate() }}><CircleArrowUp size={16} />检查更新</button>
        </div>
      </details>
    </header>
  )
}
