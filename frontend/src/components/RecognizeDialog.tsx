import { useRef, useState, useEffect } from 'react'
import { Download, FileUp, Globe2, Link } from 'lucide-react'
import { ApiError, createTask, fetchManifestTracks, isDuplicateUrlError, recognizeUrl, uploadTorrent, type ManifestTrackOption } from '../api'
import { recognitionCandidateViews, recognitionView, type RecognitionResult } from '../recognition'
import type { Settings, Task } from '../types'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'
import ConfirmDialog from './ConfirmDialog'
import { Button, DialogFooter, DialogHeader, Field, Input } from './ui'

export default function RecognizeDialog({ settings, initialUrl = '', onClose, onAdded, onNeedExtension }: { settings: Settings; initialUrl?: string; onClose: () => void; onAdded: (task?: Task) => void; onNeedExtension: () => void }) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  const [url, setUrl] = useState(initialUrl)
  const [filename, setFilename] = useState('')
  const [concurrency, setConcurrency] = useState(settings.default_concurrency || 12)
  const [checksum, setChecksum] = useState('')
  const [showDownloadOptions, setShowDownloadOptions] = useState(false)
  const [advanced, setAdvanced] = useState(false)
  const [referer, setReferer] = useState(settings.default_referer || '')
  const [origin, setOrigin] = useState(settings.default_origin || '')
  const [userAgent, setUserAgent] = useState(settings.default_user_agent || '')
  const [cookie, setCookie] = useState(settings.default_cookie || '')
  const [result, setResult] = useState<RecognitionResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [startingCandidate, setStartingCandidate] = useState('')
  const [error, setError] = useState('')
  const [duplicatePrompt, setDuplicatePrompt] = useState<{ message: string; candidate: string } | null>(null)
  const [trackChoice, setTrackChoice] = useState<{ candidate: string; format: string; video: ManifestTrackOption[]; audio: ManifestTrackOption[] } | null>(null)
  const [selectedVideo, setSelectedVideo] = useState('')
  const [selectedAudio, setSelectedAudio] = useState('')
  const torrentInput = useRef<HTMLInputElement>(null)

  const taskPayload = (candidate: string, allowDuplicate = false, video = '', audio = '') => ({
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
    selected_video: video,
    selected_audio: audio,
  })

  const startCandidate = async (candidate: string, allowDuplicate = false, video = '', audio = '', skipTrackProbe = false) => {
    // A manifest with multiple renditions gets a one-step chooser first;
    // failures or single-rendition manifests download immediately as before.
    if (!skipTrackProbe && !video && !audio && ['hls', 'dash'].includes(directType(candidate))) {
      try {
        const tracks = await fetchManifestTracks({ url: candidate, referer, origin, user_agent: userAgent, cookie })
        if ((tracks.video?.length || 0) > 1 || (tracks.audio?.length || 0) > 1) {
          setTrackChoice({ candidate, format: tracks.format, video: tracks.video || [], audio: tracks.audio || [] })
          setSelectedVideo('')
          setSelectedAudio('')
          return
        }
      } catch {
        // Track listing is best-effort only.
      }
    }
    try {
      await createTask(taskPayload(candidate, allowDuplicate, video, audio))
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

  const confirmTrackChoice = async () => {
    if (!trackChoice) return
    setBusy(true)
    setError('')
    try {
      await startCandidate(trackChoice.candidate, false, selectedVideo, selectedAudio, true)
      setTrackChoice(null)
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : '添加失败')
    } finally {
      setBusy(false)
    }
  }

  const trackLabel = (option: ManifestTrackOption) => {
    const parts = [
      option.height ? `${option.height}p` : '',
      option.bandwidth ? `${(option.bandwidth / 1_000_000).toFixed(1)} Mbps` : '',
      (option.codecs || '').split('.', 1)[0],
    ].filter(Boolean)
    return parts.join(' · ') || option.id
  }

  const downloadCandidate = async (candidate: string) => {
    setBusy(true)
    setStartingCandidate(candidate)
    setError('')
    try {
      await startCandidate(candidate)
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : '添加失败')
    } finally {
      setBusy(false)
      setStartingCandidate('')
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

  const recognize = async (target?: string) => {
    const value = (target ?? url).trim()
    if (!value) return
    setBusy(true); setError(''); setResult(null); setDuplicatePrompt(null)
    try {
      if (directType(value)) {
        await startCandidate(value)
        return
      }
      const found = await recognizeUrl({ url: value, referer, origin, user_agent: userAgent, cookie })
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
  const candidateViews = result ? recognitionCandidateViews(result.candidates) : []
  const recommendedCandidate = candidateViews.find(candidate => candidate.recommended)
  const submitWith = (value: string) => recognize(value)
  const submit = () => view?.mode === 'choose' && recommendedCandidate
    ? downloadCandidate(recommendedCandidate.url)
    : recognize()

  const importTorrent = async (file?: File) => {
    if (!file) return
    setBusy(true); setError('')
    try { const task = await uploadTorrent(file, filename); onAdded(task); onClose() }
    catch (reason: any) { setError(reason.message || '种子文件导入失败') }
    finally { setBusy(false) }
  }

  return (
    <>
      <div className="recognize-popover-backdrop" onMouseDown={onClose} />
      <section className="recognize-popover" role="dialog" aria-label="新建下载">
          <DialogHeader title="新建下载" description="支持普通文件、HLS、DASH、magnet 和 .torrent" onClose={onClose} />
          <section className="download-entry-surface">
            <Field label="下载链接" htmlFor="recognize-url">
              <div className="url-entry">
                <Link size={18} />
                <Input id="recognize-url" autoFocus value={url} onChange={event => { setUrl(event.target.value); setResult(null); setError('') }} onKeyDown={event => { if (event.key === 'Enter') void submit() }} onPaste={event => {
                  // 粘贴即识别：粘贴的完整链接直接进入识别/下载流程。
                  const pasted = event.clipboardData.getData('text').trim()
                  if (!pasted || busy) return
                  event.preventDefault()
                  setUrl(pasted)
                  setResult(null)
                  setError('')
                  window.setTimeout(() => { void submitWith(pasted) }, 0)
                }} placeholder="粘贴文件、m3u8、mpd、网页或 magnet 链接" />
              </div>
            </Field>
            <div className="recognize-quick-actions">
              <input ref={torrentInput} type="file" accept=".torrent,application/x-bittorrent" hidden onChange={event => void importTorrent(event.target.files?.[0])} />
              <Button variant="ghost" className="text-button" disabled={busy} onClick={() => torrentInput.current?.click()}><FileUp size={14} />导入 .torrent</Button>
              <span>磁力链接直接粘贴即可</span>
            </div>
          </section>
          <Button variant="ghost" className="text-button recognize-options-toggle" onClick={() => setShowDownloadOptions(value => !value)}>{showDownloadOptions ? '收起下载选项' : '下载选项'}</Button>
          {showDownloadOptions && <div className="recognize-options">
            <div className="form-row">
              <Field label="输出文件名" htmlFor="recognize-filename">
                <Input id="recognize-filename" value={filename} onChange={event => setFilename(event.target.value)} placeholder="自动生成" />
              </Field>
              <Field label="并发" htmlFor="recognize-concurrency" className="short-field">
                <Input id="recognize-concurrency" type="number" min={1} max={256} value={concurrency} onChange={event => setConcurrency(Math.max(1, Math.min(256, Number(event.target.value))))} />
              </Field>
            </div>
            <Field label="校验和" htmlFor="recognize-checksum" help="可选；下载完成后核对。多文件 BT 不支持单一校验和。">
              <Input id="recognize-checksum" value={checksum} onChange={event => setChecksum(event.target.value)} placeholder="SHA-256、SHA-1 或 MD5" />
            </Field>
            <Button variant="ghost" className="text-button" onClick={() => setAdvanced(value => !value)}>{advanced ? '收起请求上下文' : '请求上下文（Cookie / Referer）'}</Button>
          </div>}
          {showDownloadOptions && advanced && <div className="advanced-grid request-options">
            <div className="request-field"><label htmlFor="recognize-referer">Referer</label><Input id="recognize-referer" value={referer} onChange={event => setReferer(event.target.value)} placeholder={REQUEST_EXAMPLES.referer} /><small>{REQUEST_FIELD_HELP.referer}</small></div>
            <div className="request-field"><label htmlFor="recognize-origin">Origin</label><Input id="recognize-origin" value={origin} onChange={event => setOrigin(event.target.value)} placeholder={REQUEST_EXAMPLES.origin} /><small>{REQUEST_FIELD_HELP.origin}</small></div>
            <div className="request-field"><label htmlFor="recognize-ua">User-Agent</label><Input id="recognize-ua" value={userAgent} onChange={event => setUserAgent(event.target.value)} placeholder={REQUEST_EXAMPLES.userAgent} /><small>{REQUEST_FIELD_HELP.userAgent}</small></div>
            <div className="request-field"><label htmlFor="recognize-cookie">Cookie</label><Input id="recognize-cookie" value={cookie} onChange={event => setCookie(event.target.value)} placeholder="sessionid=abc; token=xyz" /><small>{REQUEST_FIELD_HELP.cookie}</small></div>
          </div>}
          {error && <div className="inline-error" role="alert">{error}</div>}
          {trackChoice && (
            <section className="track-choice" aria-label="选择清晰度与音轨">
              <strong>选择要下载的轨道</strong>
              <div className="track-choice-grid">
                {trackChoice.video.length > 1 && (
                  <label>清晰度
                    <select value={selectedVideo} onChange={event => setSelectedVideo(event.target.value)}>
                      <option value="">自动（最高清晰度）</option>
                      {trackChoice.video.map(option => (
                        <option key={option.id} value={option.id}>{trackLabel(option)}</option>
                      ))}
                    </select>
                  </label>
                )}
                {trackChoice.audio.length > 1 && (
                  <label>音轨
                    <select value={selectedAudio} onChange={event => setSelectedAudio(event.target.value)}>
                      <option value="">自动（最高码率）</option>
                      {trackChoice.audio.map(option => (
                        <option key={option.id} value={option.id}>{option.lang || option.id}{option.bandwidth ? ` · ${Math.round(option.bandwidth / 1000)} kbps` : ''}</option>
                      ))}
                    </select>
                  </label>
                )}
              </div>
              <div className="track-choice-actions">
                <Button variant="default" disabled={busy} onClick={() => void confirmTrackChoice()}>{busy ? '正在添加…' : '开始下载'}</Button>
                <Button variant="ghost" disabled={busy} onClick={() => setTrackChoice(null)}>取消</Button>
              </div>
            </section>
          )}
          {view?.mode === 'choose' && (
            <section className="candidate-list" aria-labelledby="candidate-list-title">
              <div className="candidate-list-heading">
                <div>
                  <strong id="candidate-list-title">发现 {candidateViews.length} 个候选媒体资源</strong>
                  <span>{view.message || '已结合清晰度和链接特征排序，可直接下载推荐项。'}</span>
                </div>
              </div>
              <div className="candidate-options">
                {candidateViews.map((candidate, index) => (
                  <article className={`candidate-item${candidate.recommended ? ' is-recommended' : ''}`} key={`${candidate.url}-${index}`}>
                    <div className="candidate-main">
                      <div className="candidate-description">
                        <div className="candidate-title-line">
                          <strong title={candidate.filename}>{candidate.filename}</strong>
                          {candidate.recommended && <span className="candidate-recommendation">推荐</span>}
                        </div>
                        <div className="candidate-meta">
                          <span title={candidate.host}><Globe2 size={13} aria-hidden="true" />{candidate.host}</span>
                          <span>{candidate.qualityLabel}</span>
                          <span>{candidate.sourceLabel}</span>
                        </div>
                      </div>
                      <Button
                        variant="secondary"
                        size="sm"
                        className="candidate-download"
                        disabled={busy}
                        aria-label={`下载 ${candidate.filename}，来源 ${candidate.host}`}
                        onClick={() => void downloadCandidate(candidate.url)}
                      >
                        <Download size={14} aria-hidden="true" />
                        {startingCandidate === candidate.url ? '正在添加...' : '下载此项'}
                      </Button>
                    </div>
                    <details className="candidate-technical">
                      <summary>查看技术链接</summary>
                      <code>{candidate.url}</code>
                    </details>
                  </article>
                ))}
              </div>
            </section>
          )}
          {view?.mode === 'not-found' && <div className="not-found"><p>{view.message}</p><Button variant="secondary" className="secondary-button" onClick={onNeedExtension}>打开浏览器插件工具</Button></div>}
          <DialogFooter>
            <Button variant="secondary" className="secondary-button" onClick={onClose}>取消</Button>
            <Button className="primary-button" disabled={busy || !url.trim()} onClick={() => void submit()}>
              {busy ? '正在处理...' : view?.mode === 'choose' && recommendedCandidate ? '下载推荐项' : directType(url.trim()) ? '开始下载' : '识别并下载'}
            </Button>
          </DialogFooter>
      </section>
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
