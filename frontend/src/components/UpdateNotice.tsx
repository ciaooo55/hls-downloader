import { useEffect, useState } from 'react'
import { Download, ExternalLink, X } from 'lucide-react'
import { fetchUpdateInfo, installUpdate } from '../api'
import type { UpdateInfo } from '../types'


export default function UpdateNotice() {
  const [info, setInfo] = useState<UpdateInfo | null>(null)
  const [hidden, setHidden] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    fetchUpdateInfo().then(setInfo).catch(() => {})
  }, [])

  if (!info?.available || hidden) return null

  const update = async () => {
    if (!info.can_auto_install) {
      window.open(info.release_url, '_blank', 'noopener')
      return
    }
    if (!window.confirm(`下载安装 v${info.latest_version}？下载器将在安装前自动关闭。`)) return
    setInstalling(true)
    setError('')
    try {
      await installUpdate()
    } catch (reason: any) {
      setError(reason.message || '更新失败')
      setInstalling(false)
    }
  }

  return <div className="update-notice">
    <div><strong>发现新版本 v{info.latest_version}</strong><span>{error || `当前版本 v${info.current_version}`}</span></div>
    <button className="secondary-button" disabled={installing} onClick={update}>
      {info.can_auto_install ? <Download size={15} /> : <ExternalLink size={15} />}
      {installing ? '正在下载安装…' : info.can_auto_install ? '立即更新' : '打开下载页'}
    </button>
    <button className="icon-button" title="暂时忽略" onClick={() => setHidden(true)}><X size={16} /></button>
  </div>
}
