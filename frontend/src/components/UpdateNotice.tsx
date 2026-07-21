import { useEffect, useState } from 'react'
import { Download, ExternalLink, X } from 'lucide-react'
import { fetchUpdateInfo, installUpdate } from '../api'
import type { UpdateInfo } from '../types'

const dismissKey = (version: string) => `hls_update_dismissed_${version}`

export default function UpdateNotice() {
  const [info, setInfo] = useState<UpdateInfo | null>(null)
  const [hidden, setHidden] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [phase, setPhase] = useState('')
  const [error, setError] = useState('')
  const [confirming, setConfirming] = useState(false)

  useEffect(() => {
    fetchUpdateInfo().then(value => {
      if (value?.available && localStorage.getItem(dismissKey(value.latest_version)) === '1') {
        setHidden(true)
      }
      setInfo(value)
    }).catch(() => {})
  }, [])

  if (!info?.available || hidden) return null

  const dismiss = () => {
    localStorage.setItem(dismissKey(info.latest_version), '1')
    setHidden(true)
  }

  const update = async () => {
    if (!info.can_auto_install) {
      window.open(info.release_url, '_blank', 'noopener')
      return
    }
    if (!confirming) {
      setConfirming(true)
      return
    }
    setInstalling(true)
    setPhase('正在下载安装包…')
    setError('')
    try {
      await installUpdate()
      setPhase('安装程序已启动，本窗口即将关闭…')
    } catch (reason: any) {
      setError(reason.message || '更新失败')
      setInstalling(false)
      setPhase('')
      setConfirming(false)
    }
  }

  return <div className="update-notice" role="status">
    <div>
      <strong>发现新版本 v{info.latest_version}</strong>
      <span>{error || phase || `当前 v${info.current_version} · 一键升级，任务与视频保留`}</span>
      {confirming && !installing && <span className="update-confirm-hint">确认后将关闭下载器并启动安装程序</span>}
    </div>
    <div className="update-notice-actions">
      {confirming && !installing && <button className="secondary-button" onClick={() => setConfirming(false)}>再想想</button>}
      <button className="secondary-button" disabled={installing} onClick={update}>
        {info.can_auto_install ? <Download size={15} /> : <ExternalLink size={15} />}
        {installing ? '更新中…' : confirming ? '确认更新' : info.can_auto_install ? '立即更新' : '打开下载页'}
      </button>
      <button className="icon-button" title="此版本暂不提醒" onClick={dismiss} disabled={installing}><X size={16} /></button>
    </div>
  </div>
}
