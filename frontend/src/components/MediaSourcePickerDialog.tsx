import { useMemo, useState } from 'react'
import { FileVideo, Link, ScreenShare, Tv, X } from 'lucide-react'
import { pickLocalMediaFile } from '../desktop'
import { Button, Dialog, DialogOverlay } from './ui'

export type MediaSourceSelection =
  | { source: 'local'; path: string; filename: string }
  | { source: 'url'; url: string; filename: string }

type Mode = 'cast' | 'tvbox'

function filenameFromUrl(value: string): string {
  try {
    const name = decodeURIComponent(new URL(value).pathname.split('/').filter(Boolean).pop() || '')
    return name || '网页视频'
  } catch {
    return '网页视频'
  }
}

function validMediaUrl(value: string): boolean {
  try {
    const parsed = new URL(value)
    return parsed.protocol === 'https:' || parsed.protocol === 'http:'
  } catch {
    return false
  }
}

export default function MediaSourcePickerDialog({ mode, onChoose, onClose }: {
  mode: Mode
  onChoose: (source: MediaSourceSelection) => void
  onClose: () => void
}) {
  const [source, setSource] = useState<'local' | 'url'>('local')
  const [url, setUrl] = useState('')
  const [filename, setFilename] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const verb = mode === 'cast' ? '投屏' : 'TVBox 推送'
  const inferredFilename = useMemo(() => filenameFromUrl(url.trim()), [url])

  const chooseLocal = async () => {
    setBusy(true)
    setError('')
    try {
      const result = await pickLocalMediaFile()
      if (result.canceled) return
      if (!result.ok || !result.path) throw new Error(result.error || '无法选择本机文件')
      const selectedPath = result.path
      onChoose({
        source: 'local',
        path: selectedPath,
        filename: selectedPath.split(/[\\/]/).pop() || selectedPath,
      })
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '无法选择本机文件')
    } finally {
      setBusy(false)
    }
  }

  const chooseUrl = () => {
    const value = url.trim()
    if (!validMediaUrl(value)) {
      setError('请输入设备可访问的 http:// 或 https:// 媒体链接')
      return
    }
    onChoose({ source: 'url', url: value, filename: filename.trim() || inferredFilename })
  }

  return <DialogOverlay onClose={onClose}><Dialog className="media-source-picker" label={`选择${verb}内容`}>
    <header>
      <div>
        <h2>选择{verb}内容</h2>
        <p>先选择媒体来源，再选择要发送到的设备。</p>
      </div>
      <button className="modal-close-button" type="button" title="关闭" aria-label="关闭" onClick={onClose}><X size={18} /></button>
    </header>
    <div className="media-source-options" role="radiogroup" aria-label="媒体来源">
      <button type="button" role="radio" aria-checked={source === 'local'} className={source === 'local' ? 'selected' : ''} onClick={() => { setSource('local'); setError('') }}>
        <FileVideo size={19} /><span><b>本机文件</b><small>选择已下载或电脑中的媒体文件</small></span>
      </button>
      <button type="button" role="radio" aria-checked={source === 'url'} className={source === 'url' ? 'selected' : ''} onClick={() => { setSource('url'); setError('') }}>
        <Link size={19} /><span><b>媒体链接</b><small>直接发送设备可访问的网页媒体地址</small></span>
      </button>
    </div>
    {source === 'local' ? <div className="media-source-note"><FileVideo size={17} /><span>将通过本机临时媒体服务共享所选文件，下载完成的任务请在其右键菜单中直接选择投屏或推送。</span></div> : <div className="media-source-form">
      <label><span>媒体链接</span><input autoFocus type="url" value={url} placeholder="https://example.com/video.mp4" onChange={event => { setUrl(event.target.value); setError('') }} /></label>
      <label><span>显示名称（可选）</span><input value={filename} placeholder={inferredFilename} onChange={event => setFilename(event.target.value)} /></label>
      <p>设备将直接访问该链接；需要 Cookie、登录或短效签名的链接可能无法被电视播放。</p>
    </div>}
    {error && <p className="media-source-error" role="alert">{error}</p>}
    <footer>
      <Button variant="secondary" className="secondary-button" onClick={onClose}>取消</Button>
      {source === 'local'
        ? <Button disabled={busy} onClick={() => void chooseLocal()}><FileVideo size={16} />{busy ? '正在选择…' : '选择本机文件'}</Button>
        : <Button disabled={!url.trim()} onClick={chooseUrl}>{mode === 'cast' ? <ScreenShare size={16} /> : <Tv size={16} />}继续选择设备</Button>}
    </footer>
  </Dialog></DialogOverlay>
}
