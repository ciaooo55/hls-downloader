import { useRef, useState, useEffect } from 'react'
import { FileUp, Link } from 'lucide-react'
import { ApiError, createTask, isDuplicateUrlError, recognizeUrl, uploadTorrent } from '../api'
import { recognitionView, type RecognitionResult } from '../recognition'
import type { Settings } from '../types'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'
import ConfirmDialog from './ConfirmDialog'
import { Button, Dialog, DialogFooter, DialogHeader, DialogOverlay, Field, Input } from './ui'

export default function RecognizeDialog({ settings, initialUrl = '', onClose, onAdded, onNeedExtension }: { settings: Settings; initialUrl?: string; onClose: () => void; onAdded: () => void; onNeedExtension: () => void }) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  const [url, setUrl] = useState(initialUrl)
  const [filename, setFilename] = useState('')
  const [concurrency, setConcurrency] = useState(settings.default_concurrency || 12)
  const [checksum, setChecksum] = useState('')
  const [advanced, setAdvanced] = useState(false)
  const [referer, setReferer] = useState(settings.default_referer || '')
  const [origin, setOrigin] = useState(settings.default_origin || '')
  const [userAgent, setUserAgent] = useState(settings.default_user_agent || '')
  const [cookie, setCookie] = useState(settings.default_cookie || '')
  const [result, setResult] = useState<RecognitionResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [duplicatePrompt, setDuplicatePrompt] = useState<{ message: string; candidate: string } | null>(null)
  const torrentInput = useRef<HTMLInputElement>(null)

  const taskPayload = (candidate: string, allowDuplicate = false) => ({
    url: candidate,
    task_type: 'auto' as const,
    filename,
    concurrency,
    checksum,
    referer,
    origin,
    user_agent: userAgent,
    cookie,
    allow_duplicate: allowDuplicate,
  })

  const startCandidate = async (candidate: string, allowDuplicate = false) => {
    try {
      await createTask(taskPayload(candidate, allowDuplicate))
      onAdded()
      onClose()
    } catch (reason: unknown) {
      if (!allowDuplicate && isDuplicateUrlError(reason)) {
        setDuplicatePrompt({ message: reason.message || '下载列表中已有相同链接', candidate })
        return
      }
      throw reason
    }
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
    setBusy(true); setError(''); setResult(null); setDuplicatePrompt(null)
    try {
      if (directType(url.trim())) {
        await startCandidate(url.trim())
        return
      }
      const found = await recognizeUrl({ url: url.trim(), referer, origin, user_agent: userAgent, cookie })
      setResult(found)
      if (recognitionView(found).mode === 'ready') await startCandidate(found.candidates[0].url)
    } catch (reason: unknown) {
      if (reason instanceof ApiError) setError(reason.message)
      else setError(reason instanceof Error ? reason.message : '识别失败')
    } finally {
      setBusy(false)
    }
  }

  const view = result ? recognitionView(result) : null

  const importTorrent = async (file?: File) => {
    if (!file) return
    setBusy(true); setError('')
    try { await uploadTorrent(file, filename); onAdded(); onClose() }
    catch (reason: any) { setError(reason.message || '种子文件导入失败') }
    finally { setBusy(false) }
  }

  return (
    <>
      <DialogOverlay onClose={onClose}>
        <Dialog className="recognize-modal" label="新建下载" onClose={onClose}>
          <DialogHeader title="新建下载" description="支持普通文件、HLS、DASH、magnet 和 .torrent" onClose={onClose} />
          <Field label="链接" htmlFor="recognize-url">
            <div className="url-entry">
              <Link size={18} />
              <Input id="recognize-url" autoFocus value={url} onChange={event => setUrl(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') void recognize() }} placeholder="粘贴文件、m3u8、mpd、网页或 magnet 链接" />
            </div>
          </Field>
          <div className="torrent-import">
            <input ref={torrentInput} type="file" accept=".torrent,application/x-bittorrent" hidden onChange={event => void importTorrent(event.target.files?.[0])} />
            <Button variant="secondary" className="secondary-button" disabled={busy} onClick={() => torrentInput.current?.click()}><FileUp size={15} />导入 .torrent</Button>
            <span>磁力链接可直接粘贴到上方</span>
          </div>
          <div className="form-row">
            <Field label="输出文件名（可选）" htmlFor="recognize-filename">
              <Input id="recognize-filename" value={filename} onChange={event => setFilename(event.target.value)} placeholder="自动生成" />
            </Field>
            <Field label="并发" htmlFor="recognize-concurrency" className="short-field">
              <Input id="recognize-concurrency" type="number" min={1} max={256} value={concurrency} onChange={event => setConcurrency(Math.max(1, Math.min(256, Number(event.target.value))))} />
            </Field>
          </div>
          <Field label="校验和（可选）" htmlFor="recognize-checksum" help="仅在最终文件写入后核对；不匹配会保留文件并标记失败。多文件 BT 不支持单一校验和。">
            <Input id="recognize-checksum" value={checksum} onChange={event => setChecksum(event.target.value)} placeholder="SHA-256、SHA-1 或 MD5；可写 sha256:..." />
          </Field>
          <Button variant="ghost" className="text-button" onClick={() => setAdvanced(value => !value)}>{advanced ? '收起请求选项' : '请求选项（Referer / Origin / Cookie）'}</Button>
          {advanced && <div className="advanced-grid request-options">
            <div className="request-field"><label htmlFor="recognize-referer">Referer</label><Input id="recognize-referer" value={referer} onChange={event => setReferer(event.target.value)} placeholder={REQUEST_EXAMPLES.referer} /><small>{REQUEST_FIELD_HELP.referer}</small></div>
            <div className="request-field"><label htmlFor="recognize-origin">Origin</label><Input id="recognize-origin" value={origin} onChange={event => setOrigin(event.target.value)} placeholder={REQUEST_EXAMPLES.origin} /><small>{REQUEST_FIELD_HELP.origin}</small></div>
            <div className="request-field"><label htmlFor="recognize-ua">User-Agent</label><Input id="recognize-ua" value={userAgent} onChange={event => setUserAgent(event.target.value)} placeholder={REQUEST_EXAMPLES.userAgent} /><small>{REQUEST_FIELD_HELP.userAgent}</small></div>
            <div className="request-field"><label htmlFor="recognize-cookie">Cookie</label><Input id="recognize-cookie" value={cookie} onChange={event => setCookie(event.target.value)} placeholder="sessionid=abc; token=xyz" /><small>{REQUEST_FIELD_HELP.cookie}</small></div>
          </div>}
          {error && <div className="inline-error" role="alert">{error}</div>}
          {view?.mode === 'choose' && <div className="candidate-list"><strong>发现 {result?.candidates.length} 个播放清单</strong>{result?.candidates.map(candidate => <Button key={candidate.url} variant="secondary" className="secondary-button" title={candidate.url} onClick={() => void startCandidate(candidate.url).catch((reason: unknown) => setError(reason instanceof Error ? reason.message : '添加失败'))}>{candidate.url}</Button>)}</div>}
          {view?.mode === 'not-found' && <div className="not-found"><p>{view.message}</p><Button variant="secondary" className="secondary-button" onClick={onNeedExtension}>打开浏览器插件工具</Button></div>}
          <DialogFooter>
            <Button variant="secondary" className="secondary-button" onClick={onClose}>取消</Button>
            <Button className="primary-button" disabled={busy || !url.trim()} onClick={() => void recognize()}>{busy ? '正在处理...' : directType(url.trim()) ? '开始下载' : '识别并下载'}</Button>
          </DialogFooter>
        </Dialog>
      </DialogOverlay>
      {duplicatePrompt && (
        <ConfirmDialog
          title="检测到重复下载"
          message={`${duplicatePrompt.message}\n仍可继续添加为新任务。`}
          confirmLabel="仍要下载"
          onCancel={() => setDuplicatePrompt(null)}
          onConfirm={() => {
            const candidate = duplicatePrompt.candidate
            setDuplicatePrompt(null)
            setBusy(true)
            void startCandidate(candidate, true)
              .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : '添加失败'))
              .finally(() => setBusy(false))
          }}
        />
      )}
    </>
  )
}
