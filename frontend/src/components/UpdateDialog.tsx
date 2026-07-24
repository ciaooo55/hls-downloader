import { useEffect, useState } from 'react'
import { Download, ExternalLink, FolderOpen, RefreshCw, X } from 'lucide-react'
import { fetchUpdateInfo, installUpdate, openExplorer } from '../api'
import type { UpdateInfo } from '../types'
import { friendlyUpdateError } from '../updateError'

const RELEASES_URL = 'https://github.com/ciaooo55/hls-downloader/releases/latest'

export default function UpdateDialog({ onClose }: { onClose: () => void }) {
  const [info, setInfo] = useState<UpdateInfo | null>(null)
  const [checking, setChecking] = useState(true)
  const [installing, setInstalling] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const [phase, setPhase] = useState('')
  const [error, setError] = useState('')
  const [checkWarning, setCheckWarning] = useState('')
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape' && !installing) onClose() }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose, installing])

  const check = async (force = true) => {
    setChecking(true)
    setCheckWarning('')
    setConfirming(false)
    try {
      setInfo(await fetchUpdateInfo(force))
    } catch (reason: any) {
      setCheckWarning(friendlyUpdateError(reason, '暂时无法检查更新，请稍后重试。'))
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
    if (!confirming) {
      setConfirming(true)
      return
    }
    setInstalling(true)
    setPhase('正在从 GitHub 下载安装包…')
    setError('')
    setCheckWarning('')
    try {
      await installUpdate()
      setPhase('安装程序已启动，下载器即将退出…')
    } catch (reason: any) {
      setError(friendlyUpdateError(reason, '安装包下载或启动失败，请稍后重试。'))
      setInstalling(false)
      setPhase('')
      setConfirming(false)
    }
  }

  return <div className="modal-overlay" onMouseDown={onClose}>
    <section className="modal update-modal" onMouseDown={event => event.stopPropagation()}>
      <header>
        <div><h2>软件更新</h2><p>检查、下载并安装最新版 · 任务与已下载文件会保留</p></div>
        <button className="icon-button" title="关闭" onClick={onClose} disabled={installing}><X size={18} /></button>
      </header>

      <div className="update-version-row">
        <div><span>当前版本</span><strong>{info ? `v${info.current_version}` : '--'}</strong></div>
        <div><span>最新版本</span><strong>{info ? `v${info.latest_version}` : '--'}</strong></div>
        <span className={`update-state ${info?.available ? 'available' : ''}`}>
          {checking ? '正在检查' : info?.available ? '发现新版本' : info ? '已是最新版' : '检查失败'}
        </span>
      </div>

      {info?.notes && <div className="update-notes"><b>更新说明</b><pre>{info.notes.slice(0, 600)}{info.notes.length > 600 ? '…' : ''}</pre></div>}
      {info?.download_directory && <div className="update-path">
        <div><span>安装包下载目录</span><strong title={info.download_directory}>{info.download_directory}</strong></div>
        <button className="icon-button bordered" title="打开下载目录" onClick={() => openExplorer(info.download_directory)}><FolderOpen size={17} /></button>
      </div>}
      <p className="field-note">自动更新完成后会删除下载的安装包。视频文件与任务历史不会被删除。</p>
      {phase && <div className="inline-message update-phase">{phase}</div>}
      {confirming && !installing && <div className="inline-message">确认后下载器会关闭并启动安装程序，请保存当前操作。</div>}
      {checkWarning && info?.available && <div className="inline-message update-warning" role="status">
        无法刷新更新信息，正在使用上次已验证的 v{info.latest_version} 信息。可以直接安装，或稍后重新检查。
      </div>}
      {checkWarning && !info?.available && <div className="inline-error" role="alert">{checkWarning}</div>}
      {error && <div className="inline-error" role="alert">{error}</div>}

      <footer>
        <button className="secondary-button" disabled={checking || installing} onClick={() => check(true)}><RefreshCw size={15} />{checking ? '检查中…' : '重新检查'}</button>
        {!info && !checking && <button className="secondary-button" onClick={() => window.open(RELEASES_URL, '_blank', 'noopener')}><ExternalLink size={15} />打开 Release 页面</button>}
        {confirming && !installing && <button className="secondary-button" onClick={() => setConfirming(false)}>取消</button>}
        {info?.available && <button className="primary-button" disabled={installing || checking} onClick={update}>
          {info.can_auto_install ? <Download size={15} /> : <ExternalLink size={15} />}
          {installing ? '更新中…' : confirming ? '确认并安装' : info.can_auto_install ? '下载并安装' : '打开下载页'}
        </button>}
      </footer>
    </section>
  </div>
}
