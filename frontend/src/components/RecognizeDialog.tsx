import { useRef, useState } from 'react'
import { FileUp, Link, X } from 'lucide-react'
import { createTask, recognizeUrl, uploadTorrent } from '../api'
import { recognitionView, type RecognitionResult } from '../recognition'
import type { Settings } from '../types'
import { LEGACY_REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'

export default function RecognizeDialog({ settings, initialUrl = '', onClose, onAdded, onNeedUserscript }: { settings: Settings; initialUrl?: string; onClose: () => void; onAdded: () => void; onNeedUserscript: () => void }) {
  const [url, setUrl] = useState(initialUrl)
  const [filename, setFilename] = useState('')
  const [concurrency, setConcurrency] = useState(settings.default_concurrency || 8)
  const [advanced, setAdvanced] = useState(false)
  const [referer, setReferer] = useState(settings.default_referer || '')
  const [origin, setOrigin] = useState(settings.default_origin || '')
  const [userAgent, setUserAgent] = useState(settings.default_user_agent || '')
  const [cookie, setCookie] = useState(settings.default_cookie || '')
  const [result, setResult] = useState<RecognitionResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const torrentInput = useRef<HTMLInputElement>(null)

  const startCandidate = async (candidate: string) => {
    await createTask({ url: candidate, task_type: 'auto', filename, concurrency, referer, origin, user_agent: userAgent, cookie })
    onAdded(); onClose()
  }
  const directType = (value: string) => {
    if (value.toLowerCase().startsWith('magnet:')) return 'torrent'
    try {
      const path = new URL(value).pathname.toLowerCase()
      if (path.endsWith('.m3u8')) return 'hls'
      if (path.endsWith('.mpd')) return 'dash'
      if (path.endsWith('.torrent')) return 'torrent'
      if (/\.(mp4|mkv|webm|mov|mp3|m4a|flac|zip|7z|rar|exe|msi|pdf|iso)$/.test(path)) return 'http'
    } catch {}
    return ''
  }
  const recognize = async () => {
    if (!url.trim()) return
    setBusy(true); setError(''); setResult(null)
    try {
      if (directType(url.trim())) {
        await startCandidate(url.trim())
        return
      }
      const found = await recognizeUrl({ url: url.trim(), referer, origin, user_agent: userAgent, cookie })
      setResult(found)
      if (recognitionView(found).mode === 'ready') await startCandidate(found.candidates[0].url)
    } catch (reason: any) { setError(reason.message || '识别失败') }
    finally { setBusy(false) }
  }
  const view = result ? recognitionView(result) : null
  const importTorrent = async (file?: File) => {
    if (!file) return
    setBusy(true); setError('')
    try { await uploadTorrent(file, filename); onAdded(); onClose() }
    catch (reason: any) { setError(reason.message || '种子文件导入失败') }
    finally { setBusy(false) }
  }

  return <div className="modal-overlay" onMouseDown={onClose}><section className="modal recognize-modal" onMouseDown={event => event.stopPropagation()}>
    <header><div><h2>新建下载</h2><p>支持普通文件、HLS、DASH、magnet 和 .torrent</p></div><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <label>链接</label><div className="url-entry"><Link size={18} /><input autoFocus value={url} onChange={event => setUrl(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') recognize() }} placeholder="粘贴文件、m3u8、mpd、网页或 magnet 链接" /></div>
    <div className="torrent-import"><input ref={torrentInput} type="file" accept=".torrent,application/x-bittorrent" hidden onChange={event => importTorrent(event.target.files?.[0])} /><button className="secondary-button" disabled={busy} onClick={() => torrentInput.current?.click()}><FileUp size={15} />导入 .torrent</button><span>磁力链接可直接粘贴到上方</span></div>
    <div className="form-row"><div><label>输出文件名（可选）</label><input value={filename} onChange={event => setFilename(event.target.value)} placeholder="自动生成" /></div><div className="short-field"><label>并发</label><input type="number" min={1} max={64} value={concurrency} onChange={event => setConcurrency(Math.max(1, Math.min(64, Number(event.target.value))))} /></div></div>
    <button className="text-button" onClick={() => setAdvanced(value => !value)}>{advanced ? '收起请求选项' : '请求选项（Referer / Origin / Cookie）'}</button>
    {advanced && <div className="advanced-grid request-options">
      <div className="request-field"><label>Referer</label><input value={referer} onChange={event => setReferer(event.target.value)} placeholder={LEGACY_REQUEST_EXAMPLES.referer} /><small>{REQUEST_FIELD_HELP.referer}</small></div>
      <div className="request-field"><label>Origin</label><input value={origin} onChange={event => setOrigin(event.target.value)} placeholder={LEGACY_REQUEST_EXAMPLES.origin} /><small>{REQUEST_FIELD_HELP.origin}</small></div>
      <div className="request-field"><label>User-Agent</label><input value={userAgent} onChange={event => setUserAgent(event.target.value)} placeholder={LEGACY_REQUEST_EXAMPLES.userAgent} /><small>{REQUEST_FIELD_HELP.userAgent}</small></div>
      <div className="request-field"><label>Cookie</label><input value={cookie} onChange={event => setCookie(event.target.value)} placeholder="sessionid=abc; token=xyz" /><small>{REQUEST_FIELD_HELP.cookie}</small></div>
    </div>}
    {error && <div className="inline-error">{error}</div>}
    {view?.mode === 'choose' && <div className="candidate-list"><strong>发现 {result?.candidates.length} 个播放清单</strong>{result?.candidates.map(candidate => <button key={candidate.url} title={candidate.url} onClick={() => startCandidate(candidate.url)}>{candidate.url}</button>)}</div>}
    {view?.mode === 'not-found' && <div className="not-found"><p>{view.message}</p><button className="secondary-button" onClick={onNeedUserscript}>打开浏览器脚本工具</button></div>}
    <footer><button className="secondary-button" onClick={onClose}>取消</button><button className="primary-button" disabled={busy || !url.trim()} onClick={recognize}>{busy ? '正在处理...' : directType(url.trim()) ? '开始下载' : '识别并下载'}</button></footer>
  </section></div>
}
