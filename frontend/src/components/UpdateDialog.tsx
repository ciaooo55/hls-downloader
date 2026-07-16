import { useEffect, useState } from 'react'
import { Download, ExternalLink, FolderOpen, RefreshCw, X } from 'lucide-react'
import { fetchUpdateInfo, installUpdate, openExplorer } from '../api'
import type { UpdateInfo } from '../types'

export default function UpdateDialog({ onClose }: { onClose: () => void }) {
  const [info, setInfo] = useState<UpdateInfo | null>(null)
  const [checking, setChecking] = useState(true)
  const [installing, setInstalling] = useState(false)
  const [error, setError] = useState('')

  const check = async (force = true) => {
    setChecking(true)
    setError('')
    try {
      setInfo(await fetchUpdateInfo(force))
    } catch (reason: any) {
      setError(reason.message || '检查更新失败')
    } finally {
      setChecking(false)
    }
  }

  useEffect(() => { void check(false) }, [])

  const update = async () => {
    if (!info?.available) return
    if (!info.can_auto_install) {
      window.open(info.release_url, '_blank', 'noopener')
      return
    }
    if (!window.confirm(`下载安装 v${info.latest_version}？安装程序启动后下载器会自动关闭。`)) return
    setInstalling(true)
    setError('')
    try {
      await installUpdate()
    } catch (reason: any) {
      setError(reason.message || '更新失败')
      setInstalling(false)
    }
  }

  return <div className="modal-overlay" onMouseDown={onClose}>
    <section className="modal update-modal" onMouseDown={event => event.stopPropagation()}>
      <header>
        <div><h2>软件更新</h2><p>检查并安装 HLS Downloader 新版本</p></div>
        <button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button>
      </header>

      <div className="update-version-row">
        <div><span>当前版本</span><strong>{info ? `v${info.current_version}` : '--'}</strong></div>
        <div><span>最新版本</span><strong>{info ? `v${info.latest_version}` : '--'}</strong></div>
        <span className={`update-state ${info?.available ? 'available' : ''}`}>
          {checking ? '正在检查' : info?.available ? '发现新版本' : info ? '已是最新版' : '检查失败'}
        </span>
      </div>

      {info?.download_directory && <div className="update-path">
        <div><span>安装包下载目录</span><strong title={info.download_directory}>{info.download_directory}</strong></div>
        <button className="icon-button bordered" title="打开下载目录" onClick={() => openExplorer(info.download_directory)}><FolderOpen size={17} /></button>
      </div>}
      <p className="field-note">自动更新完成后会删除下载的安装包。已下载的视频和任务历史不会被删除。</p>
      {error && <div className="inline-error">{error}</div>}

      <footer>
        <button className="secondary-button" disabled={checking || installing} onClick={() => check(true)}><RefreshCw size={15} />{checking ? '检查中…' : '重新检查'}</button>
        {info?.available && <button className="primary-button" disabled={installing} onClick={update}>
          {info.can_auto_install ? <Download size={15} /> : <ExternalLink size={15} />}
          {installing ? '正在下载…' : info.can_auto_install ? '下载并安装' : '打开下载页'}
        </button>}
      </footer>
    </section>
  </div>
}
