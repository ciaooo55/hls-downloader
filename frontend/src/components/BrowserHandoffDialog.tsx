import { Download, Globe2, Undo2 } from 'lucide-react'
import { fmtBytes } from '../format'

export interface BrowserHandoff {
  id: string
  url: string
  filename: string
  mime_type: string
  source_page_url: string
  size: number
}

export default function BrowserHandoffDialog({ item, busy, onResolve }: {
  item: BrowserHandoff
  busy: boolean
  onResolve: (action: 'accept' | 'reject') => void
}) {
  let host = item.url
  try { host = new URL(item.url).host } catch {}
  return <div className="modal-overlay browser-handoff-overlay">
    <section className="modal browser-handoff-dialog" role="dialog" aria-modal="true" aria-label="浏览器下载接管">
      <header><div><h2>接管浏览器下载</h2><p>浏览器下载已暂停，选择由谁继续</p></div></header>
      <div className="browser-handoff-body">
        <div className="browser-handoff-file"><Download size={24} /><div><strong>{item.filename || host}</strong><span>{item.mime_type || '普通文件'}{item.size ? ` · ${fmtBytes(item.size)}` : ''}</span></div></div>
        <div className="browser-handoff-source"><Globe2 size={15} /><span>{host}</span></div>
      </div>
      <footer>
        <button className="secondary-button" disabled={busy} onClick={() => onResolve('reject')}><Undo2 size={15} />交给浏览器</button>
        <button className="primary-button" disabled={busy} onClick={() => onResolve('accept')}><Download size={15} />立即下载</button>
      </footer>
    </section>
  </div>
}
