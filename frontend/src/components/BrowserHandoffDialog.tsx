import { useMemo, useState } from 'react'
import { AlertTriangle, Download, FolderOpen, Globe2, X } from 'lucide-react'
import { fmtBytes } from '../format'
import type { Settings } from '../types'
import { downloadCategory, DOWNLOAD_CATEGORY_LABELS, type DownloadCategory } from '../downloadCategory'
import FolderPicker from './FolderPicker'

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

export default function BrowserHandoffDialog({ item, busy, settings, onResolve, standalone = false }: {
  item: BrowserHandoff
  busy: boolean
  settings: Settings
  onResolve: (action: 'accept' | 'cancel', decision?: BrowserHandoffDecision) => void
  standalone?: boolean
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
  const chooseCategory = (value: DownloadCategory) => {
    setCategory(value)
    setDirectory(settings.browser_category_dirs?.[value] || settings.download_dir || '')
  }
  const topDuplicate = item.duplicates?.[0]
  const duplicateHint = item.duplicate_message || (
    topDuplicate
      ? `与“${topDuplicate.filename || '已有任务'}”${topDuplicate.status ? `（${topDuplicate.status}）` : ''}重复。仍可继续下载，也可取消。`
      : '下载列表中已有相同链接。仍可继续下载，也可取消。'
  )
  return <div className={`modal-overlay browser-handoff-overlay${standalone ? ' browser-handoff-standalone' : ''}`}>
    <section className="modal browser-handoff-dialog" role="dialog" aria-modal="true" aria-label="浏览器下载接管">
      <header><div><h2>下载文件信息</h2><p>浏览器已暂停，由 HLS Downloader 接管</p></div><button className="modal-close-button" title="取消下载" disabled={busy} onClick={() => onResolve('cancel')}><X size={18} /></button></header>
      <div className="browser-handoff-body">
        {item.duplicate && <div className="browser-handoff-duplicate" role="status">
          <AlertTriangle size={16} />
          <div>
            <strong>下载列表中已有相同链接</strong>
            <span>{duplicateHint}</span>
          </div>
        </div>}
        <div className="browser-handoff-file"><Download size={20} /><div><strong>{filename || host}</strong><span>{item.mime_type || '类型未知'}{item.size ? ` · ${fmtBytes(item.size)}` : ' · 大小未知'}</span></div></div>
        <div className="browser-handoff-source"><Globe2 size={14} /><span>{host}</span></div>
        <label>文件名</label><input value={filename} onChange={event => setFilename(event.target.value)} />
        <label>分类</label><div className="handoff-categories">{(['media', 'program', 'archive', 'other'] as DownloadCategory[]).map(value => <button key={value} className={category === value ? 'active' : ''} onClick={() => chooseCategory(value)}>{DOWNLOAD_CATEGORY_LABELS[value]}</button>)}</div>
        <label>保存到</label><div className="path-bar"><input value={directory} onChange={event => setDirectory(event.target.value)} /><button className="icon-button bordered" title="选择保存文件夹" onClick={() => setShowPicker(true)}><FolderOpen size={16} /></button></div>
        <label className="checkbox-label"><input type="checkbox" checked={remember} onChange={event => setRemember(event.target.checked)} />记住“{DOWNLOAD_CATEGORY_LABELS[category]}”文件的保存位置</label>
      </div>
      <footer>
        <button className="secondary-button" disabled={busy} onClick={() => onResolve('cancel')}><X size={15} />取消</button>
        <button className="primary-button" disabled={busy || !filename.trim() || !directory.trim()} onClick={() => onResolve('accept', { filename: filename.trim(), download_dir: directory.trim(), category, remember })}><Download size={15} />开始下载</button>
      </footer>
      {showPicker && <FolderPicker initialPath={directory} onSelect={path => { setDirectory(path); setShowPicker(false) }} onClose={() => setShowPicker(false)} />}
    </section>
  </div>
}