import { useRef, useState, useEffect } from 'react'
import { Download, FileUp, Globe2, Link } from 'lucide-react'
import { ApiError, createTask, fetchManifestTracks, isDuplicateUrlError, recognizeUrl, uploadTorrent, type ManifestTrackOption } from '../api'
import { recognitionCandidateViews, recognitionView, type RecognitionResult } from '../recognition'
import type { Settings, Task } from '../types'
import { parseRequestHeaders, REQUEST_EXAMPLES, REQUEST_FIELD_HELP, sourcePageRequestContext } from '../requestHelp'
import { parseCurlCommand } from '../curlImport'
import ConfirmDialog from './ConfirmDialog'
import { Button, DialogFooter, DialogHeader, Field, Input } from './ui'

function encodeRequestBody(value: string): string {
  const bytes = new TextEncoder().encode(value)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}

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
  const [scheduledStartAt, setScheduledStartAt] = useState('')
  const [scheduledStopAt, setScheduledStopAt] = useState('')
  const [completionAction, setCompletionAction] = useState<'none' | 'shutdown' | 'sleep' | 'hibernate'>('none')
  const [showDownloadOptions, setShowDownloadOptions] = useState(false)
  const [advanced, setAdvanced] = useState(false)
  const [referer, setReferer] = useState(settings.default_referer || '')
  const [origin, setOrigin] = useState(settings.default_origin || '')
  const [userAgent, setUserAgent] = useState(settings.default_user_agent || '')
  const [cookie, setCookie] = useState(settings.default_cookie || '')
  const [headersText, setHeadersText] = useState('')
  const [sourcePageUrl, setSourcePageUrl] = useState('')
  const [requestMethod, setRequestMethod] = useState('GET')
  const [requestBody, setRequestBody] = useState('')
  const [curlNotice, setCurlNotice] = useState('')
  const [result, setResult] = useState<RecognitionResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [startingCandidate, setStartingCandidate] = useState('')
  const [error, setError] = useState('')
  const [duplicatePrompt, setDuplicatePrompt] = useState<{ message: string; candidate: string; video: string; audio: string } | null>(null)
  const [trackChoice, setTrackChoice] = useState<{ candidate: string; format: string; video: ManifestTrackOption[]; audio: ManifestTrackOption[] } | null>(null)
  const [selectedVideo, setSelectedVideo] = useState('')
  const [selectedAudio, setSelectedAudio] = useState('')
  const torrentInput = useRef<HTMLInputElement>(null)

  const contextFor = () => ({
    referer,
    origin,
    userAgent,
    cookie,
    requestHeaders: parseRequestHeaders(headersText),
  })

  const applySourcePageContext = () => {
    const context = sourcePageRequestContext(sourcePageUrl)
    if (!context) {
      setError('来源网页 URL 必须是有效的 HTTP(S) 地址')
      return
    }
    setReferer(context.referer)
    setOrigin(context.origin)
    setError('')
  }

  const taskPayload = (candidate: string, allowDuplicate = false, video = '', audio = '') => {
    const context = contextFor()
    return {
      url: candidate,
      task_type: 'auto' as const,
      source_page_url: sourcePageUrl.trim(),
      filename,
      concurrency,
      checksum,
      referer: context.referer,
      origin: context.origin,
      user_agent: context.userAgent,
      cookie: context.cookie,
      request_headers: context.requestHeaders,
      request_method: requestMethod,
      request_body: requestMethod === 'POST' && requestBody ? encodeRequestBody(requestBody) : '',
      allow_duplicate: allowDuplicate,
      selected_video: video,
      selected_audio: audio,
      scheduled_start_at: scheduledStartAt || null,
      scheduled_stop_at: scheduledStopAt || null,
      completion_action: completionAction,
    }
  }

  const startCandidate = async (candidate: string, allowDuplicate = false, video = '', audio = '', skipTrackProbe = false) => {
    const context = contextFor()
    // A manifest with multiple renditions gets a one-step chooser first;
    // failures or single-rendition manifests download immediately as before.
    if (!skipTrackProbe && !video && !audio && ['hls', 'dash'].includes(directType(candidate))) {
      try {
        const tracks = await fetchManifestTracks({ url: candidate, referer: context.referer, origin: context.origin, user_agent: context.userAgent, cookie: context.cookie, request_headers: context.requestHeaders })
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
        // Keep the chosen tracks (and skip re-probing) so confirming does
        // not loop back into the chooser or silently fall back to auto.
        setDuplicatePrompt({ message: reason.message || '下载列表中已有相同链接', candidate, video, audio })
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
      const context = contextFor()
      const found = await recognizeUrl({ url: value, referer: context.referer, origin: context.origin, user_agent: context.userAgent, cookie: context.cookie, request_headers: context.requestHeaders })
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
                  try {
                    const imported = parseCurlCommand(pasted)
                    if (imported) {
                      event.preventDefault()
                      setUrl(imported.url)
                      setReferer(imported.referer)
                      setOrigin(imported.origin)
                      if (imported.userAgent) setUserAgent(imported.userAgent)
                      setCookie(imported.cookie)
                      setHeadersText(Object.entries(imported.headers).map(([name, value]) => `${name}: ${value}`).join('\n'))
                      setRequestMethod(imported.method)
                      setRequestBody(imported.body)
                      setAdvanced(true)
                      setShowDownloadOptions(true)
                      setResult(null)
                      setCurlNotice(`已导入 cURL ${imported.method} 请求，请确认后开始下载`)
                      setError('')
                      return
                    }
                  } catch (reason) {
                    event.preventDefault()
                    setError(reason instanceof Error ? reason.message : 'cURL 命令无法解析')
                    return
                  }
                  event.preventDefault()
                  setUrl(pasted)
                  setResult(null)
                  setError('')
                  window.setTimeout(() => { void submitWith(pasted) }, 0)
                }} placeholder="粘贴链接、magnet 或浏览器“复制为 cURL”" />
              </div>
            </Field>
            <div className="recognize-quick-actions">
              <input ref={torrentInput} type="file" accept=".torrent,application/x-bittorrent" hidden onChange={event => void importTorrent(event.target.files?.[0])} />
              <Button variant="ghost" className="text-button" disabled={busy} onClick={() => torrentInput.current?.click()}><FileUp size={14} />导入 .torrent</Button>
              <span>{curlNotice || '磁力链接和“复制为 cURL”可直接粘贴'}</span>
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
            <div className="form-row">
              <Field label="计划开始" htmlFor="recognize-scheduled-start" help="可选；到本机时间后自动开始，关闭程序后计划仍会保留。">
                <Input id="recognize-scheduled-start" type="datetime-local" value={scheduledStartAt} onChange={event => setScheduledStartAt(event.target.value)} />
              </Field>
              <Field label="计划停止" htmlFor="recognize-scheduled-stop" help="可选；直播会停止并合并，普通下载会安全暂停。">
                <Input id="recognize-scheduled-stop" type="datetime-local" value={scheduledStopAt} onChange={event => setScheduledStopAt(event.target.value)} />
              </Field>
            </div>
            <Field label="完成后动作" htmlFor="recognize-completion-action" help="执行前会显示 30 秒倒计时，可随时取消；仅影响这一项任务。">
              <select id="recognize-completion-action" value={completionAction} onChange={event => setCompletionAction(event.target.value as typeof completionAction)}><option value="none">无</option><option value="shutdown">关机</option><option value="sleep">睡眠</option><option value="hibernate">休眠</option></select>
            </Field>
            <Button variant="ghost" className="text-button" onClick={() => setAdvanced(value => !value)}>{advanced ? '收起请求上下文' : '请求上下文（Cookie / Referer）'}</Button>
          </div>}
          {showDownloadOptions && advanced && <div className="advanced-grid request-options">
            <div className="request-field request-context-source"><label htmlFor="recognize-source-page">来源网页 URL</label><div><Input id="recognize-source-page" value={sourcePageUrl} onChange={event => setSourcePageUrl(event.target.value)} placeholder="浏览器地址栏中的播放网页，不是 m3u8/CDN 链接" /><Button type="button" variant="secondary" size="sm" onClick={applySourcePageContext}>填入 Referer / Origin</Button></div><small>按当前来源网页 URL 填入 Referer 与 Origin。Cookie、User-Agent 和其他请求头仍由下方字段或浏览器扩展提供。</small></div>
            <div className="request-field"><label htmlFor="recognize-referer">Referer</label><Input id="recognize-referer" value={referer} onChange={event => setReferer(event.target.value)} placeholder={REQUEST_EXAMPLES.referer} /><small>{REQUEST_FIELD_HELP.referer}</small></div>
            <div className="request-field"><label htmlFor="recognize-origin">Origin</label><Input id="recognize-origin" value={origin} onChange={event => setOrigin(event.target.value)} placeholder={REQUEST_EXAMPLES.origin} /><small>{REQUEST_FIELD_HELP.origin}</small></div>
            <div className="request-field"><label htmlFor="recognize-ua">User-Agent</label><Input id="recognize-ua" value={userAgent} onChange={event => setUserAgent(event.target.value)} placeholder={REQUEST_EXAMPLES.userAgent} /><small>{REQUEST_FIELD_HELP.userAgent}</small></div>
            <div className="request-field"><label htmlFor="recognize-cookie">Cookie</label><Input id="recognize-cookie" value={cookie} onChange={event => setCookie(event.target.value)} placeholder="sessionid=abc; token=xyz" /><small>{REQUEST_FIELD_HELP.cookie}</small></div>
            <div className="request-field request-context-headers"><label htmlFor="recognize-headers">其他请求头</label><textarea id="recognize-headers" value={headersText} onChange={event => setHeadersText(event.target.value)} placeholder={'每行一个，例如：\nAuthorization: Bearer ...\nX-Playback-Token: ...'} /><small>可按实际网站请求填写；留空不添加。Cookie 请单独填写，不能写在这里。</small></div>
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
            const { candidate, video, audio } = duplicatePrompt
            setDuplicatePrompt(null)
            setBusy(true)
            // skipTrackProbe: the tracks were already chosen (or left auto).
            void startCandidate(candidate, true, video, audio, true)
              .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : '添加失败'))
              .finally(() => setBusy(false))
          }}
        />
      )}
    </>
  )
}
