import { useState } from 'react'
import { CheckCircle2, FolderOpen, FolderSearch } from 'lucide-react'
import { fmtBytes } from '../format'
import { needsExecutableConfirm, type DownloadCompleteItem } from '../downloadOverlay'
import { Button } from './ui'

export default function DownloadCompletePanel({
  item,
  remaining,
  busy,
  error,
  onOpenFile,
  onOpenFolder,
  onClose,
}: {
  item: DownloadCompleteItem
  remaining: number
  busy: boolean
  error?: string
  onOpenFile: (confirmed: boolean) => void
  onOpenFolder: () => void
  onClose: () => void
}) {
  const [confirmOpen, setConfirmOpen] = useState(false)
  const executable = needsExecutableConfirm(item.output_path)
  const displayName = item.filename || item.title || item.id

  const openFile = () => {
    if (executable && !confirmOpen) {
      setConfirmOpen(true)
      return
    }
    onOpenFile(executable)
  }

  return (
    <section className="download-complete-panel">
      <div className="download-complete-hero">
        <CheckCircle2 size={28} />
        <div>
          <strong>下载完成</strong>
          <span>{remaining > 0 ? `还有 ${remaining} 个已完成文件` : '文件已保存到本机'}</span>
        </div>
      </div>
      <p className="download-complete-name" title={item.output_path || displayName}>{displayName}</p>
      <p className="download-complete-meta">
        {item.downloaded_bytes > 0 ? fmtBytes(item.downloaded_bytes) : '已保存'}
        {item.output_path ? ` · ${item.output_path}` : ''}
      </p>
      {confirmOpen && (
        <p className="download-complete-warn">即将运行从互联网下载的可执行文件。仅在信任来源时继续。</p>
      )}
      {error && <p className="download-complete-error">{error}</p>}
      <footer>
        <Button type="button" variant="secondary" disabled={busy || !item.output_path} onClick={onOpenFolder}>
          <FolderSearch size={15} />打开目录
        </Button>
        <Button type="button" disabled={busy || !item.output_path || !item.output_is_file} onClick={openFile}>
          <FolderOpen size={15} />{confirmOpen ? '仍然打开' : '打开'}
        </Button>
        <Button type="button" variant="ghost" disabled={busy} onClick={onClose}>关闭</Button>
      </footer>
    </section>
  )
}
