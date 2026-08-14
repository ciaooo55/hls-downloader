import { useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, Download, FolderOpen, Globe2, ShieldCheck } from 'lucide-react'
import { fmtBytes } from '../format'
import { openTaskInExplorer, taskAction } from '../api'
import { duplicateActionLabel } from '../duplicateTask'
import type { Settings } from '../types'
import { downloadCategory, DOWNLOAD_CATEGORY_LABELS, type DownloadCategory } from '../downloadCategory'
import { pickFolder } from '../desktop'
import { formatRequestHeaders, parseRequestHeaders } from '../requestHelp'
import FolderPicker from './FolderPicker'
import { Button, Dialog, DialogFooter, DialogHeader, DialogOverlay, Input } from './ui'

export interface BrowserHandoffDuplicate {
  id: string
  status: string
  filename: string
  output_path?: string
  updated_at?: string
  suggested_action?: 'resume' | 'retry' | 'start' | 'open' | 'focus' | 'none'
}

export interface BrowserHandoff {
  id: string
  url: string
  filename: string
  title?: string
  mime_type: string
  source_page_url: string
  size: number
  resource_kind?: 'hls' | 'dash' | 'media' | 'file' | 'magnet'
  status?: string
  duplicate?: boolean
  duplicates?: BrowserHandoffDuplicate[]
  duplicate_message?: string
  effective_context?: {
    target_origin?: string
    referer?: string
    origin?: string
    user_agent?: string
    cookie?: string
    request_headers?: Record<string, string>
  }
}

export interface BrowserHandoffDecision {
  filename: string
  download_dir: string
  category: DownloadCategory
  remember: boolean
  cookie?: string
  request_headers?: Record<string, string>
}

export interface BrowserHandoffCancelDecision {
  suppress_site_kind: true
}

export default function BrowserHandoffDialog({ item, busy, settings, onResolve, standalone = false, queueRemaining = 0 }: {
  item: BrowserHandoff
  busy: boolean
  settings: Settings
  onResolve: (action: 'accept' | 'cancel', decision?: BrowserHandoffDecision | BrowserHandoffCancelDecision) => void
  standalone?: boolean
  queueRemaining?: number
}) {
  let host = item.url
  try { host = new URL(item.url).host } catch {}
  const initialCategory = useMemo(() => downloadCategory(item.filename || item.url, item.mime_type), [item])
  const fallbackName = decodeURIComponent(item.url.split(/[?#]/, 1)[0].split('/').pop() || 'download')
  const [filename, setFilename] = useState(item.filename || fallbackName)
  const [category, setCategory] = useState<DownloadCategory>(initialCategory)
  const [directory, setDirectory] = useState(settings.browser_category_dirs?.[initialCategory] || settings.download_dir || '')
  const seededDirectory = useRef(Boolean((settings.browser_category_dirs?.[initialCategory] || settings.download_dir || '').trim()))
  const [remember, setRemember] = useState(true)
  const [showPicker, setShowPicker] = useState(false)
  const [contextOpen, setContextOpen] = useState(false)
  const [cookie, setCookie] = useState(() => item.effective_context?.cookie || '')
  const [headersText, setHeadersText] = useState(() => formatRequestHeaders(item.effective_context?.request_headers))
  const [suppressArmed, setSuppressArmed] = useState(false)
  const canAccept = Boolean(filename.trim() && directory.trim() && !busy)
  const directoryLabel = directory.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || directory || '未设置保存位置'
  let sourceHost = ''
  try { sourceHost = new URL(item.source_page_url).hostname } catch {}
  const resourceKindLabel = ({ hls: 'HLS 视频', dash: 'DASH 视频', media: '媒体文件', magnet: '磁力链接', file: '文件' } as const)[item.resource_kind || 'file']

  useEffect(() => {
    if (seededDirectory.current) return
    const next = settings.browser_category_dirs?.[category] || settings.download_dir || ''
    if (!next.trim()) return
    setDirectory(next)
    seededDirectory.current = true
  }, [settings, category])

  const chooseCategory = (value: DownloadCategory) => {
    setCategory(value)
    setDirectory(settings.browser_category_dirs?.[value] || settings.download_dir || '')
  }

  const accept = () => {
    if (!canAccept) return
    onResolve('accept', decision())
  }

  const decision = (): BrowserHandoffDecision => ({
    filename: filename.trim(),
    download_dir: directory.trim(),
    category,
    remember,
    cookie: cookie.trim(),
    request_headers: parseRequestHeaders(headersText),
  })

  const cancel = () => {
    if (busy) return
    onResolve('cancel')
  }

  const suppressSiteKind = () => {
    if (busy || !sourceHost) return
    if (!suppressArmed) {
      setSuppressArmed(true)
      return
    }
    onResolve('cancel', { suppress_site_kind: true })
  }

  const openDirectoryPicker = async () => {
    if (busy) return
    const native = await pickFolder(directory)
    if (native.ok && native.path) {
      setDirectory(native.path)
      return
    }
    if (native.canceled) return
    setShowPicker(true)
  }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      const typing = Boolean(target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable))
      if (event.key === 'Escape') {
        event.preventDefault()
        if (busy) return
        if (showPicker) {
          setShowPicker(false)
          return
        }
        onResolve('cancel')
        return
      }
      if (event.key === 'Enter' && !typing && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault()
        if (!filename.trim() || !directory.trim() || busy) return
        onResolve('accept', decision())
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [busy, filename, directory, category, remember, cookie, headersText, showPicker, onResolve])

  const topDuplicate = item.duplicates?.[0]
  const duplicateHint = item.duplicate_message || (
    topDuplicate
      ? `与“${topDuplicate.filename || '已有任务'}”${topDuplicate.status ? `（${topDuplicate.status}）` : ''}重复。仍可继续下载，也可取消。`
      : '下载列表中已有相同链接。仍可继续下载，也可取消。'
  )

  return <>
    <DialogOverlay className={`browser-handoff-overlay${standalone ? ' browser-handoff-standalone' : ''}`}>
      <Dialog className="browser-handoff-dialog" label="浏览器下载接管">
        <DialogHeader
          title="浏览器下载"
          description={`确认后加入下载队列${queueRemaining > 0 ? ` · 还有 ${queueRemaining} 个待确认` : ''}`}
          onClose={standalone || busy ? undefined : cancel}
        />
        <div className="browser-handoff-body">
          {item.duplicate && <div className="browser-handoff-duplicate" role="status">
            <AlertTriangle size={16} />
            <div>
              <strong>下载列表中已有相同链接</strong>
              <span>{duplicateHint}</span>
            </div>
          </div>}
          <section className="browser-handoff-summary">
            <div className="browser-handoff-file"><Download size={20} /><div><strong>{filename || host}</strong><span>{item.mime_type || '类型未知'}{item.size ? ` · ${fmtBytes(item.size)}` : ' · 大小未知'}</span></div></div>
            <div className="browser-handoff-source"><Globe2 size={14} /><span title={item.url}>{host}</span></div>
          </section>
          {sourceHost && <button type="button" className={`browser-handoff-suppress${suppressArmed ? ' armed' : ''}`} disabled={busy} onClick={suppressSiteKind}>
            {suppressArmed
              ? `再次点击：不再自动提示 ${sourceHost} 的${resourceKindLabel}`
              : `不再自动提示 ${sourceHost} 的${resourceKindLabel}`}
          </button>}
          <details className="browser-handoff-options">
            <summary title={`保存为${DOWNLOAD_CATEGORY_LABELS[category]} · ${directory || directoryLabel}`}>
              <span>保存选项</span><small>{DOWNLOAD_CATEGORY_LABELS[category]} · {directoryLabel}</small>
            </summary>
            <div className="browser-handoff-option-fields">
              <label htmlFor="handoff-filename">文件名</label>
              <Input id="handoff-filename" value={filename} onChange={event => setFilename(event.target.value)} disabled={busy} />
              <label>分类</label>
              <div className="handoff-categories">{(['media', 'program', 'archive', 'other'] as DownloadCategory[]).map(value => (
                <Button key={value} type="button" variant={category === value ? 'default' : 'secondary'} className={category === value ? 'active' : ''} size="sm" disabled={busy} onClick={() => chooseCategory(value)}>{DOWNLOAD_CATEGORY_LABELS[value]}</Button>
              ))}</div>
              <label htmlFor="handoff-directory">保存到</label>
              <div className="path-bar">
                <Input id="handoff-directory" value={directory} onChange={event => setDirectory(event.target.value)} disabled={busy} />
                <Button type="button" variant="ghost" size="icon" className="icon-button bordered" title="选择保存文件夹" disabled={busy} onClick={() => void openDirectoryPicker()}><FolderOpen size={16} /></Button>
              </div>
              <label className="checkbox-label">
                <input type="checkbox" checked={remember} disabled={busy} onChange={event => setRemember(event.target.checked)} />
                记住“{DOWNLOAD_CATEGORY_LABELS[category]}”文件的保存位置
              </label>
            </div>
          </details>
          <details className="browser-handoff-context" open={contextOpen} onToggle={event => setContextOpen((event.currentTarget as HTMLDetailsElement).open)}>
            <summary>
              <ShieldCheck size={14} />
              <span>网站请求上下文</span>
              <small>默认使用来源网页</small>
            </summary>
            <div className="browser-handoff-context-fields">
              <p>来源网页：<code title={item.source_page_url}>{item.source_page_url || '未捕获'}</code></p>
              <p>实际下载域：<code title={item.effective_context?.target_origin || item.url}>{item.effective_context?.target_origin || host}</code>。下面已填入本次实际捕获值，可直接修改；留空则保留默认来源上下文。</p>
              <label htmlFor="handoff-cookie">Cookie（本次实际值，可编辑）</label>
              <Input id="handoff-cookie" value={cookie} onChange={event => setCookie(event.target.value)} disabled={busy} placeholder="未捕获 Cookie；留空使用默认上下文" />
              <label htmlFor="handoff-headers">请求头（本次实际值，可编辑，每行：Header: value）</label>
              <textarea id="handoff-headers" value={headersText} onChange={event => setHeadersText(event.target.value)} disabled={busy} placeholder={'未捕获请求头；留空使用默认上下文\nReferer: https://example.com/page\nAuthorization: Bearer …'} />
            </div>
          </details>
        </div>
        <DialogFooter>
          {item.duplicate && topDuplicate && duplicateActionLabel(topDuplicate.suggested_action) ? <Button type="button" variant="secondary" className="secondary-button" disabled={busy} onClick={() => void (async () => { const action = topDuplicate.suggested_action || 'none'; if (action === 'open') await openTaskInExplorer(topDuplicate.id); else if (action !== 'focus' && action !== 'none') await taskAction(topDuplicate.id, action); onResolve('cancel') })()}>{duplicateActionLabel(topDuplicate.suggested_action)}</Button> : null}<Button type="button" variant="secondary" className="secondary-button" disabled={busy} onClick={cancel}>取消</Button>
          <Button type="button" className="primary-button" disabled={!canAccept} onClick={accept}><Download size={15} />{busy ? '处理中…' : '确认下载'}</Button>
        </DialogFooter>
      </Dialog>
    </DialogOverlay>
    {showPicker && <FolderPicker initialPath={directory} onSelect={path => { setDirectory(path); setShowPicker(false) }} onClose={() => setShowPicker(false)} />}
  </>
}
