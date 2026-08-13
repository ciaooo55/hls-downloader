import { useEffect, useState } from 'react'
import { AlertTriangle, Bookmark, FileText, FolderOpen, LoaderCircle, MonitorPlay, Pause, PlayCircle, RotateCcw, ScreenShare, ShieldCheck, Trash2, Tv, X, XCircle } from 'lucide-react'
import { getFailureDetails } from '../failureDetails'
import { fmtBytes, fmtDate, fmtEta, fmtSpeed } from '../format'
import { getDisplayedProgress, isActiveTransfer } from '../taskState'
import { stageLabel, taskStatusLabel } from '../taskPresentation'
import type { Task } from '../types'
import { canSaveSiteProfile } from '../taskContextActions'
import { fetchTorrentFiles, refreshTaskRequest, selectTorrentFiles, setTaskSpeedLimit } from '../api'
import { Button, Dialog, DialogOverlay } from './ui'
import { redactUrlForDiagnostics } from '../diagnosticUrl'
import SpeedChart from './SpeedChart'
import ConnectionMap from './ConnectionMap'

export default function TaskDetailsModal({ task, pending, onClose, onLog, onAction, onOpenFile, onLaunchFile, onPushToTv, onCast, onPreview }: {
  task: Task
  pending: boolean
  onClose: () => void
  onLog: () => void
  onAction: (action: string) => void
  onOpenFile: () => void
  onLaunchFile: () => void
  onPushToTv: () => void
  onCast: () => void
  onPreview: () => void
}) {
  const failure = task.error_message || task.error_code ? getFailureDetails(task) : null
  const actions = task.available_actions || []
  const activeTransfer = isActiveTransfer(task.status)
  const [torrentFiles, setTorrentFiles] = useState<Array<{ index: number; path: string; size: number }>>([])
  const [selectedFiles, setSelectedFiles] = useState<number[]>([])
  const [selectionBusy, setSelectionBusy] = useState(false)
  const [selectionNotice, setSelectionNotice] = useState('')
  const [diagnosticsCopied, setDiagnosticsCopied] = useState(false)
  const [limitDraft, setLimitDraft] = useState(String(task.speed_limit_kib || 0))
  const [limitBusy, setLimitBusy] = useState(false)
  const [limitNotice, setLimitNotice] = useState('')
  const [limitFocused, setLimitFocused] = useState(false)
  const [showRequestRefresh, setShowRequestRefresh] = useState(false)
  const [requestUrl, setRequestUrl] = useState(task.url)
  const [requestCookie, setRequestCookie] = useState('')
  const [requestBusy, setRequestBusy] = useState(false)
  const [requestNotice, setRequestNotice] = useState('')
  const applySpeedLimit = async () => {
    const value = Math.max(0, Math.min(1048576, Math.round(Number(limitDraft) || 0)))
    setLimitBusy(true)
    setLimitNotice('')
    try {
      await setTaskSpeedLimit(task.id, value)
      setLimitDraft(String(value))
      setLimitNotice(value > 0 ? `已限速 ${value} KiB/s，立即生效` : '已取消该任务的限速')
    } catch {
      setLimitNotice('保存限速失败，请检查连接后重试')
    } finally {
      setLimitBusy(false)
    }
  }
  useEffect(() => {
    const close = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [onClose])
  useEffect(() => {
    if (task.task_type !== 'torrent') return
    fetchTorrentFiles(task.id).then(result => {
      setTorrentFiles(result.files || [])
      setSelectedFiles(result.selected || [])
    }).catch(() => {})
  }, [task.id, task.task_type, task.status])
  useEffect(() => {
    if (!limitFocused) setLimitDraft(String(task.speed_limit_kib || 0))
  }, [task.id, task.speed_limit_kib, limitFocused])
  return <DialogOverlay onClose={onClose}><Dialog className="task-details" label="任务详情">
    <header><div><h2>{task.title || task.filename || task.id}</h2><p title={task.url}>{task.url}</p></div><button className="modal-close-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <div className="task-details-body">
      <div className="detail-grid"><Detail label="类型" value={task.task_type.toUpperCase()} /><Detail label="请求" value={task.request_method || 'GET'} /><Detail label="状态" value={taskStatusLabel(task)} /><Detail label="阶段" value={stageLabel(task.stage)} /><Detail label="进度" value={`${getDisplayedProgress(task).toFixed(1)}%`} /><Detail label={task.task_type === 'hls' && task.status !== 'done' && task.total_bytes > task.downloaded_bytes ? '预计总大小' : '总大小'} value={fmtBytes(task.total_bytes)} /><Detail label="已下载" value={fmtBytes(task.downloaded_bytes)} /><Detail label="下载速度" value={activeTransfer ? fmtSpeed(task.speed_bytes_per_sec) : '--'} /><Detail label="剩余时间" value={activeTransfer ? fmtEta(task.eta_seconds) : '--'} /><Detail label={task.task_type === 'torrent' ? 'Piece' : '分片'} value={`${task.completed_segments}/${task.total_segments}`} /><Detail label={task.task_type === 'torrent' ? 'Peer / Seed' : '活动线程'} value={task.task_type === 'torrent' ? `${task.peer_count} / ${task.seed_count}` : `${task.active_workers}/${task.max_workers}`} /><Detail label="上传速度" value={task.task_type === 'torrent' && activeTransfer ? fmtSpeed(task.upload_speed_bytes_per_sec) : '--'} /><Detail label="更新时间" value={fmtDate(task.updated_at)} /></div>
      <SpeedChart samples={task.speed_history} current={activeTransfer ? task.speed_bytes_per_sec : 0} peak={task.speed_peak_bytes_per_sec} />
      <ConnectionMap parts={task.connection_parts} total={task.total_bytes} />
      {canSaveSiteProfile(task) && <section className="task-speed-limit"><button className="secondary-button" disabled={pending} onClick={() => onAction('saveSiteProfile')}><Bookmark size={16} />保存为站点规则</button><p className="field-note">把当前主机的 Cookie、目录和请求头存成站点规则；浏览器捕获的值仍优先。</p></section>}
      {task.status !== 'done' && task.task_type !== 'torrent' && <section className="task-speed-limit">
        <b>任务限速（KiB/s）</b>
        <div className="task-speed-limit-row">
          <input type="number" min={0} max={1048576} value={limitDraft} onChange={event => setLimitDraft(event.target.value)} onFocus={() => setLimitFocused(true)} onBlur={() => setLimitFocused(false)} aria-label="任务限速" />
          <button className="secondary-button" disabled={limitBusy} onClick={() => void applySpeedLimit()}>{limitBusy ? '保存中…' : '应用'}</button>
        </div>
        <p className="field-note">0 表示不限制；与全局限速同时生效，取两者更严格值。</p>
        {limitNotice && <p className="torrent-selection-notice" role="status">{limitNotice}</p>}
      </section>}
      {task.last_log && <div className="detail-message"><b>最近日志</b><code>{task.last_log}</code></div>}
      {task.request_method === 'POST' && <section className="post-replay-details"><ShieldCheck size={16} /><div><b>安全 POST 下载</b><span>此资源需要重放网页请求；为避免重复提交，下载仅使用单连接。暂停、恢复或重试会重新请求服务器，不能使用断点续传。</span></div></section>}
      {task.expected_checksum && <section className={`checksum-details ${task.checksum_verified === false ? 'failed' : task.checksum_verified ? 'verified' : ''}`}><b>文件校验</b><dl><div><dt>期望</dt><dd>{task.expected_checksum}</dd></div><div><dt>结果</dt><dd>{task.checksum_verified === true ? '已通过' : task.checksum_verified === false ? '不匹配或未能校验' : '等待下载完成'}</dd></div>{task.checksum_actual && <div><dt>实际</dt><dd>{task.checksum_actual}</dd></div>}</dl></section>}
      {task.av_scan?.state ? <section className={`checksum-details ${task.av_scan.state === 'threat' ? 'failed' : task.av_scan.state === 'clean' ? 'verified' : ''}`}><b>病毒扫描</b><dl><div><dt>结果</dt><dd>{task.av_scan.state === 'clean' ? '未发现威胁' : task.av_scan.state === 'threat' ? '发现威胁' : task.av_scan.state === 'running' ? '正在扫描' : task.av_scan.state === 'skipped' ? '已跳过' : '扫描异常'}</dd></div>{task.av_scan.engine ? <div><dt>引擎</dt><dd>{task.av_scan.engine}</dd></div> : null}{task.av_scan.detail ? <div><dt>详情</dt><dd>{task.av_scan.detail}</dd></div> : null}</dl></section> : null}
      {(task.mirrors?.length || task.mirror_status?.length) ? <section className="checksum-details"><b>备用地址</b><dl>{(task.mirror_status?.length ? task.mirror_status : (task.mirrors || []).map(url => ({ url, state: 'pending', detail: '' }))).map(item => <div key={item.url}><dt>{item.state === 'active' ? '使用中' : item.state === 'skipped' ? '已忽略' : item.state === 'failed' ? '失败' : '待探测'}</dt><dd title={item.url}>{item.url}{item.detail ? ` · ${item.detail}` : ''}</dd></div>)}</dl></section> : null}

      {failure && <section className="failure-details">
      <h3><AlertTriangle size={17} />{failure.title}
        <button className="text-button failure-copy" title="复制诊断信息，便于反馈问题" onClick={() => {
          const lines = [
            `任务: ${task.title || task.filename || task.id}`,
            `链接（已脱敏）: ${redactUrlForDiagnostics(task.url)}`,
            `状态: ${task.status}`,
            ...failure.items.map(item => `${item.label}: ${item.value}`),
            failure.message ? `失败原因: ${failure.message}` : '',
            `最近日志: ${task.last_log || '--'}`,
          ].filter(Boolean)
          void navigator.clipboard.writeText(lines.join('\n')).then(() => setDiagnosticsCopied(true))
          window.setTimeout(() => setDiagnosticsCopied(false), 2000)
        }}>{diagnosticsCopied ? '已复制' : '复制诊断'}</button>
      </h3>
      {failure.items.length > 0 && <dl>{failure.items.map(item => <div key={item.label}><dt>{item.label}</dt><dd title={item.value}>{item.value}</dd></div>)}</dl>}
      {failure.message && <div className="failure-message"><b>失败原因</b><code>{failure.message}</code></div>}
      {failure.hint && <div className="failure-hint"><b>处理建议</b><span>{failure.hint}</span></div>}
      {failure.steps && failure.steps.length > 0 && (
        <div className="failure-steps">
          <b>建议步骤</b>
          <ol>{failure.steps.map((step: string) => <li key={step}>{step}</li>)}</ol>
        </div>
      )}
      </section>}
      {task.status !== 'done' && task.task_type !== 'torrent' && <section className="task-request-refresh">
        <button className="secondary-button" onClick={() => setShowRequestRefresh(value => !value)}>{showRequestRefresh ? '收起链接更新' : '更新下载链接 / 凭据'}</button>
        {showRequestRefresh && <div className="task-request-refresh-form">
          <label><span>新的资源地址</span><textarea rows={3} value={requestUrl} onChange={event => setRequestUrl(event.target.value)} /></label>
          <label><span>新的 Cookie（留空则保留原值）</span><textarea rows={2} value={requestCookie} onChange={event => setRequestCookie(event.target.value)} placeholder="可选；从浏览器复制最新 Cookie" /></label>
          <p className="field-note">适合 403/410、短效 token 或签名过期。更新后会校验服务端文件身份，匹配时从已有字节继续，不匹配时安全重下。</p>
          <button className="primary-button" disabled={requestBusy || !requestUrl.trim()} onClick={async () => {
            setRequestBusy(true); setRequestNotice('')
            try {
              await refreshTaskRequest(task.id, { url: requestUrl.trim(), ...(requestCookie ? { cookie: requestCookie } : {}), auto_resume: true })
              setRequestCookie(''); setRequestNotice('已更新，正在从已有进度继续下载')
            } catch (error) {
              setRequestNotice(error instanceof Error ? error.message : '更新失败，请检查链接后重试')
            } finally { setRequestBusy(false) }
          }}>{requestBusy ? '正在更新…' : '更新并继续'}</button>
          {requestNotice && <p className="torrent-selection-notice" role="status">{requestNotice}</p>}
        </div>}
      </section>}
      {task.task_type === 'torrent' && torrentFiles.length > 0 && <section className="torrent-files">
      <div className="torrent-files-head"><h3>BT 文件选择</h3><span>{selectedFiles.length}/{torrentFiles.length}</span></div>
      <div className="torrent-file-list">{torrentFiles.map(file => <label key={file.index}><input type="checkbox" checked={selectedFiles.includes(file.index)} disabled={task.status === 'done'} onChange={event => { setSelectionNotice(''); setSelectedFiles(current => event.target.checked ? [...current, file.index] : current.filter(index => index !== file.index)) }} /><span title={file.path}>{file.path}</span><b>{fmtBytes(file.size)}</b></label>)}</div>
      {task.status !== 'done' && <><p className="field-note">{task.status === 'awaiting_selection' ? '种子尚未开始下载。确认文件后才会连接 Peer，避免下载不需要的内容。' : '下载中也可以更新选择；未选文件会停止请求，已下载的数据会保留在此任务中。'}</p><button className="secondary-button" disabled={selectionBusy || !selectedFiles.length} onClick={async () => { setSelectionBusy(true); setSelectionNotice(''); try { await selectTorrentFiles(task.id, selectedFiles); setSelectionNotice(task.status === 'downloading' ? '选择已生效：新增文件已排入下载，取消项不再请求；已有数据不会删除。' : '文件选择已保存，将在开始或恢复时生效。') } catch { setSelectionNotice('保存文件选择失败，请检查连接后重试。') } finally { setSelectionBusy(false) } }}>{selectionBusy ? '正在保存…' : '保存文件选择'}</button>{task.status === 'awaiting_selection' && <button className="primary-button" disabled={selectionBusy || !selectedFiles.length} onClick={async () => { setSelectionBusy(true); setSelectionNotice(''); try { await selectTorrentFiles(task.id, selectedFiles); onAction('start') } catch { setSelectionNotice('无法保存文件选择，请检查连接后重试。') } finally { setSelectionBusy(false) } }}>{selectionBusy ? '正在确认…' : '确认选择并开始下载'}</button>}{selectionNotice && <p className="torrent-selection-notice" role="status">{selectionNotice}</p>}</>}
      </section>}
      {task.output_missing && <section className="checksum-details failed"><b>最终文件已删除</b><p className="field-note">任务记录仍保留。可以从原地址重新下载，或打开所在目录查看是否被移动。</p></section>}
      {task.output_path && <div className="output-path" title={task.output_path}>{task.output_path}</div>}
    </div>
    <footer className="detail-actions">
      <button className="secondary-button" onClick={onLog}><FileText size={16} />查看日志</button>
      {!pending && actions.includes('pause') && <button className="secondary-button" onClick={() => onAction('pause')}><Pause size={16} />{task.is_live ? '停止录制' : '暂停'}</button>}
      {!pending && actions.includes('start') && task.task_type !== 'torrent' && <button className="primary-button" onClick={() => onAction('start')}><PlayCircle size={16} />开始下载</button>}
      {!pending && actions.includes('resume') && <button className="primary-button" onClick={() => onAction('resume')}><RotateCcw size={16} />{task.is_live ? '继续录制' : '恢复'}</button>}
      {!pending && actions.includes('retry') && <button className="primary-button" onClick={() => onAction('retry')}><RotateCcw size={16} />重试</button>}
      {!pending && actions.includes('cancel') && <button className="secondary-button" onClick={() => onAction('cancel')}><XCircle size={16} />取消</button>}
      {!pending && actions.includes('delete') && <button className="danger-button" onClick={() => onAction('delete')}><Trash2 size={16} />删除记录</button>}
      {!pending && actions.includes('delete_files') && <button className="danger-button" onClick={() => onAction('deleteFiles')}><Trash2 size={16} />{task.status === 'done' ? '删除任务及文件' : '停止并删除'}</button>}
      {actions.includes('open') && <button className="secondary-button" onClick={onOpenFile}><FolderOpen size={16} />所在位置</button>}
      {actions.includes('launch') && <button className="secondary-button" onClick={onLaunchFile}><PlayCircle size={16} />系统播放</button>}
      {actions.includes('cast') && <button className="secondary-button" onClick={onCast}><ScreenShare size={16} />{task.status === 'done' ? '投屏已下载文件' : '投屏当前下载'}</button>}
      {actions.includes('pushTvbox') && <button className="secondary-button" onClick={onPushToTv}><Tv size={16} />{task.status === 'done' ? 'TVBox 推送已下载文件' : 'TVBox 推送当前下载'}</button>}
      {actions.includes('preview') && <button className="primary-button" onClick={onPreview}><MonitorPlay size={16} />{task.status === 'done' ? '内置播放' : '边下边播'}</button>}
      {pending && <span className="pending-label"><LoaderCircle className="spin" size={15} />正在处理</span>}
    </footer>
  </Dialog></DialogOverlay>
}

function Detail({ label, value }: { label: string; value: string }) { return <div><span>{label}</span><strong>{value}</strong></div> }
