import { useEffect, useMemo, useState } from 'react'
import { AlertTriangle, Download, FolderOpen, Globe2 } from 'lucide-react'
import { fmtBytes } from '../format'
import type { Settings } from '../types'
import { downloadCategory, DOWNLOAD_CATEGORY_LABELS, type DownloadCategory } from '../downloadCategory'
import { pickFolder } from '../desktop'
import FolderPicker from './FolderPicker'
import { Button, Dialog, DialogFooter, DialogHeader, DialogOverlay, Input } from './ui'

export interface BrowserHandoffDuplicate {
  id: string
  status: string
  filename: string
  output_path?: string
  updated_at?: string
}

export interface BrowserHandoff {
  id: string
  url: string
  filename: string
  title?: string
  mime_type: string
  source_page_url: string
  size: number
  status?: string
  duplicate?: boolean
  duplicates?: BrowserHandoffDuplicate[]
  duplicate_message?: string
}

export interface BrowserHandoffDecision {
  filename: string
  download_dir: string
  category: DownloadCategory
  remember: boolean
}

export default function BrowserHandoffDialog({ item, busy, settings, onResolve, standalone = false, queueRemaining = 0 }: {
  item: BrowserHandoff
  busy: boolean
  settings: Settings
  onResolve: (action: 'accept' | 'cancel', decision?: BrowserHandoffDecision) => void
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
  const [remember, setRemember] = useState(true)
  const [showPicker, setShowPicker] = useState(false)
  const canAccept = Boolean(filename.trim() && directory.trim() && !busy)

  const chooseCategory = (value: DownloadCategory) => {
    setCategory(value)
    setDirectory(settings.browser_category_dirs?.[value] || settings.download_dir || '')
  }

  const accept = () => {
    if (!canAccept) return
    onResolve('accept', {
      filename: filename.trim(),
      download_dir: directory.trim(),
      category,
      remember,
    })
  }

  const cancel = () => {
    if (busy) return
    onResolve('cancel')
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
        if (!busy) onResolve('cancel')
        return
      }
      if (event.key === 'Enter' && !typing && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault()
        if (!filename.trim() || !directory.trim() || busy) return
        onResolve('accept', {
          filename: filename.trim(),
          download_dir: directory.trim(),
          category,
          remember,
        })
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [busy, filename, directory, category, remember, onResolve])

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
          title="下载文件信息"
          description={`浏览器已暂停，由 HLS Downloader 接管${queueRemaining > 0 ? ` · 还有 ${queueRemaining} 个待确认` : ''}`}
          onClose={busy ? undefined : cancel}
        />
        <div className="browser-handoff-body">
          {item.duplicate && <div className="browser-handoff-duplicate" role="status">
            <AlertTriangle size={16} />
            <div>
              <strong>下载列表中已有相同链接</strong>
              <span>{duplicateHint}</span>
            </div>
          </div>}
          <div className="browser-handoff-file"><Download size={20} /><div><strong>{filename || host}</strong><span>{item.mime_type || '类型未知'}{item.size ? ` · ${fmtBytes(item.size)}` : ' · 大小未知'}</span></div></div>
          <div className="browser-handoff-source"><Globe2 size={14} /><span title={item.url}>{host}</span></div>
          <label htmlFor="handoff-filename">文件名</label>
          <Input id="handoff-filename" value={filename} onChange={event => setFilename(event.target.value)} autoFocus disabled={busy} />
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
        <DialogFooter>
          <Button type="button" variant="secondary" className="secondary-button" disabled={busy} onClick={cancel}>取消</Button>
          <Button type="button" className="primary-button" disabled={!canAccept} onClick={accept}><Download size={15} />{busy ? '处理中…' : '开始下载'}</Button>
        </DialogFooter>
      </Dialog>
    </DialogOverlay>
    {showPicker && <FolderPicker initialPath={directory} onSelect={path => { setDirectory(path); setShowPicker(false) }} onClose={() => setShowPicker(false)} />}
  </>
}
