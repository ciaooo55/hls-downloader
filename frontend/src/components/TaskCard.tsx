import React, { useState } from 'react'
import { openExplorer } from '../api'
import ProgressBar from './ProgressBar'
import { getDisplayedProgress, isPausable, isRunningStatus } from '../taskState'

const STATUS_COLORS: Record<string, string> = {
  queued: '#6b7280', downloading: '#3b82f6', downloading_m3u8: '#3b82f6',
  parsing: '#8b5cf6', downloading_segments: '#3b82f6',
  merging: '#f59e0b', remuxing: '#a855f7',
  pausing: '#f97316',
  done: '#22c55e', failed: '#ef4444',
  paused: '#f97316', canceled: '#6b7280', unsupported: '#ef4444',
}

const STATUS_LABELS: Record<string, string> = {
  queued: '排队中', downloading: '下载中', downloading_m3u8: '获取清单',
  parsing: '解析中', downloading_segments: '下载分片',
  merging: '合并中', remuxing: '转封装中',
  pausing: '暂停中',
  done: '已完成', failed: '已失败',
  paused: '已暂停', canceled: '已取消', unsupported: '不支持',
}

const CONN_LABELS: Record<string, { text: string; color: string }> = {
  running: { text: '正常', color: '#22c55e' },
  connecting: { text: '连接中', color: '#f59e0b' },
  reconnecting: { text: '重连中', color: '#f97316' },
  idle: { text: '空闲', color: '#6b7280' },
  error: { text: '断开', color: '#ef4444' },
}

function fmtSpeed(bps: number): string {
  if (bps <= 0) return '...'
  if (bps < 1024) return `${bps.toFixed(0)} B/s`
  if (bps < 1048576) return `${(bps / 1024).toFixed(1)} KB/s`
  return `${(bps / 1048576).toFixed(1)} MB/s`
}

function fmtEta(sec: number): string {
  if (sec <= 0 || sec > 360000) return '--:--'
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = Math.floor(sec % 60)
  if (h > 0) return `${h}h ${m}m ${s}s`
  return `${m}m ${s}s`
}

function fmtBytes(b: number): string {
  if (b <= 0) return '0 B'
  const u = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0; let n = b
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++ }
  return `${n.toFixed(1)} ${u[i]}`
}

function fmtDuration(ms: number): string {
  if (ms <= 0) return '--:--'
  const sec = Math.floor(ms / 1000)
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = sec % 60
  if (h > 0) return `${h}h${m}m${s}s`
  return `${m}m${s}s`
}

interface Props {
  task: any
  busy: boolean
  onStart: () => void; onPause: () => void; onResume: () => void
  onCancel: () => void; onRetry: () => void; onDelete: () => void; onLog: () => void
}

export default function TaskCard({ task, busy, onStart, onPause, onResume, onCancel, onRetry, onDelete, onLog }: Props) {
  const t = task
  const [showDetail, setShowDetail] = useState(false)
  const isRunning = isRunningStatus(t.status)
  const isDone = t.status === 'done'
  const isFailed = t.status === 'failed'
  const isPaused = t.status === 'paused'
  const isCanceled = t.status === 'canceled'
  const isUnsupported = t.status === 'unsupported'
  const isFinished = isDone || isFailed || isCanceled || isUnsupported
  const collapsed = isFinished && !showDetail

  const pct = getDisplayedProgress(t)

  const conn = CONN_LABELS[t.connection_status] || CONN_LABELS.idle

  const duration = t.started_at && t.finished_at
    ? new Date(t.finished_at).getTime() - new Date(t.started_at).getTime()
    : t.started_at ? Date.now() - new Date(t.started_at).getTime() : 0

  // Collapsed: one-line summary for done/canceled/failed
  if (collapsed) {
    return (
      <div style={rowDone(isFailed)}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, flex: 1, minWidth: 0 }}>
          <span style={{ fontSize: 11, fontWeight: 700, color: STATUS_COLORS[t.status], minWidth: 44 }}>{STATUS_LABELS[t.status]}</span>
          <span style={{ fontSize: 13, fontWeight: 600, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{t.title || t.filename || t.id}</span>
          {isDone && <span style={{ fontSize: 12, color: '#22c55e', whiteSpace: 'nowrap' }}>{fmtBytes(t.downloaded_bytes)}</span>}
          {duration > 0 && <span style={{ fontSize: 11, color: '#6b7280', whiteSpace: 'nowrap' }}>{fmtDuration(duration)}</span>}
          {isFailed && <span style={{ fontSize: 11, color: '#f87171', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: 200 }}>{t.error_message?.includes('403') ? '403 拒绝访问' : t.error_message?.includes('timeout') ? '超时' : t.error_message?.slice(0, 40) || '失败'}</span>}
        </div>
        <div style={{ display: 'flex', gap: 4, flexShrink: 0 }}>
          <button onClick={onLog} style={btnT} title="日志">📋</button>
          {isDone && <button onClick={() => navigator.clipboard.writeText(t.output_path || '')} style={btnT} title="复制路径">📁</button>}
          {(isFailed || isCanceled || isUnsupported) && <button onClick={onRetry} disabled={busy} style={btnT} title="重试">🔄</button>}
          <button onClick={() => setShowDetail(true)} style={btnT} title="展开">▼</button>
          <button onClick={onDelete} disabled={busy} style={btnT} title="删除">🗑</button>
        </div>
      </div>
    )
  }

  return (
    <div style={card}>
      {/* Header */}
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 10 }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 4 }}>
            <span style={{ fontSize: 15, fontWeight: 700, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{t.title || t.filename || t.id}</span>
            <button onClick={() => navigator.clipboard.writeText(t.id || '')} style={btnT} title="复制 ID">ID</button>
          </div>
          <div style={{ fontSize: 11, color: '#4b5563', maxWidth: 520, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{t.url}</div>
        </div>
        <span style={{ ...badge, background: STATUS_COLORS[t.status] || '#6b7280' }}>{STATUS_LABELS[t.status] || t.status}</span>
      </div>

      {/* Stage + connection status line */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 8, fontSize: 12 }}>
        <span style={{ color: STATUS_COLORS[t.status] || '#9ca3af' }}>
          {['merging','remuxing'].includes(t.status)
            ? `${t.stage} (${(t.post_percent ?? 0).toFixed(1)}%)`
            : t.stage || ''}
        </span>
        {isRunning && (
          <>
            <span style={{ color: '#6b7280' }}>|</span>
            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
              <span style={{ width: 7, height: 7, borderRadius: '50%', background: conn.color, display: 'inline-block' }} />
              <span style={{ color: conn.color }}>{conn.text}</span>
            </span>
            <span style={{ color: '#6b7280' }}>|</span>
            <span style={{ color: '#94a3b8' }}>
              线程 {t.active_workers ?? 0}/{t.max_workers ?? t.concurrency ?? '-'}
            </span>
            {t.active_slots > 0 && (
              <>
                <span style={{ color: '#6b7280' }}>|</span>
                <span style={{ color: '#94a3b8' }}>槽位 {t.active_slots}/{t.max_workers ?? t.concurrency ?? '-'}</span>
              </>
            )}
            {t.reconnect_count > 0 && (
              <>
                <span style={{ color: '#6b7280' }}>|</span>
                <span style={{ color: '#f97316' }}>重连 {t.reconnect_count}次</span>
              </>
            )}
          </>
        )}
      </div>

      {/* Last log */}
      {t.last_log && <div style={{ fontSize: 11, color: '#6b7280', marginBottom: 8, fontFamily: 'monospace', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{t.last_log}</div>}

      {/* Progress bar */}
      <ProgressBar percent={pct} status={t.status} />

      {/* Stats row */}
      <div style={{ display: 'flex', gap: 16, fontSize: 12, color: '#9ca3af', marginTop: 8, marginBottom: 8, flexWrap: 'wrap' }}>
        <span style={{ fontWeight: 600 }}>{t.completed_segments}/{t.total_segments} 分片</span>
        {['merging','remuxing'].includes(t.status) && <span style={{ color: '#f59e0b' }}>后处理 {(t.post_percent ?? 0).toFixed(1)}%</span>}
        <span>{fmtBytes(t.downloaded_bytes)}</span>
        <span style={{ color: t.speed_bytes_per_sec > 0 ? '#22c55e' : '#6b7280', fontWeight: isRunning ? 600 : 400 }}>{fmtSpeed(t.speed_bytes_per_sec)}</span>
        <span>ETA {fmtEta(t.eta_seconds)}</span>
        {t.failed_segments > 0 && <span style={{ color: '#ef4444' }}>失败 {t.failed_segments}</span>}
        {duration > 0 && <span>耗时 {fmtDuration(duration)}</span>}
      </div>

      {/* Current active segments */}
      {showDetail && isRunning && Array.isArray(t.active_segment_indexes) && t.active_segment_indexes.length > 0 && (
        <div style={{ fontSize: 11, color: '#60a5fa', marginBottom: 8 }}>
          当前下载: [{t.active_segment_indexes.join(', ')}]
        </div>
      )}

      {/* Error box */}
      {t.error_message && (
        <div style={{ fontSize: 12, color: '#f87171', marginBottom: 8, background: '#1a1114', border: '1px solid #3b1c24', borderRadius: 8, padding: 10 }}>
          <div style={{ fontWeight: 700, marginBottom: 4 }}>❌ {t.error_message.includes('HTTP 403') ? '403 被拒绝' : t.error_message.includes('timeout') ? '网络超时' : t.error_message.includes('segments failed') ? '部分分片失败' : '下载失败'}</div>
          <div style={{ color: '#fecaca', fontFamily: 'monospace', fontSize: 11, wordBreak: 'break-all' }}>{t.error_message}</div>
        </div>
      )}

      {/* Output path */}
      {t.output_path && (
        <div style={{ fontSize: 12, color: '#94a3b8', marginBottom: 8, display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 360 }}>📁 {t.output_path}</span>
          <button onClick={() => navigator.clipboard.writeText(t.output_path || '')} style={btnT}>复制路径</button>
          <button onClick={() => navigator.clipboard.writeText(((t.output_path || '').split('\\').pop() || '').split('/').pop() || '')} style={btnT}>复制文件名</button>
          {isDone && <button onClick={() => {
            const dir = (t.output_path || '').replace(/[/\\][^/\\]+$/, '')
            navigator.clipboard.writeText(dir)
          }} style={btnT}>复制目录</button>}
          {isDone && t.output_path && <button onClick={() => openExplorer(t.output_path)} style={btnT}>打开目录</button>}
        </div>
      )}

      {/* Actions */}
      <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
        {isFinished && showDetail && <button onClick={() => setShowDetail(false)} style={btnG}>收起</button>}
        {t.status === 'queued' && <button onClick={onStart} disabled={busy} style={btnS}>▶ 开始</button>}
        {isPausable(t) && <button onClick={onPause} disabled={busy} style={btnO}>⏸ 暂停</button>}
        {isPaused && <button onClick={onResume} disabled={busy} style={btnS}>▶ 恢复</button>}
        {(isRunning || isPaused) && <button onClick={onCancel} disabled={busy} style={btnR}>⏹ 取消</button>}
        {(isFailed || isCanceled || isUnsupported) && <button onClick={onRetry} disabled={busy} style={btnS}>🔄 重试</button>}
        <button onClick={onLog} style={btnG}>📋 日志</button>
        {isFinished && <button onClick={onDelete} disabled={busy} style={btnR}>🗑 删除</button>}
      </div>
    </div>
  )
}

const card: React.CSSProperties = { background: '#1c1f26', padding: 16, borderRadius: 10, border: '1px solid #333', marginBottom: 10 }
const rowDone = (isFailed: boolean): React.CSSProperties => ({
  background: isFailed ? '#1a1114' : '#151820',
  padding: '8px 14px', borderRadius: 8,
  border: isFailed ? '1px solid #3b1c24' : '1px solid #22262e',
  marginBottom: 6, display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8,
})
const badge: React.CSSProperties = { padding: '3px 10px', borderRadius: 12, fontSize: 12, fontWeight: 600, color: '#fff', whiteSpace: 'nowrap', flexShrink: 0 }
const btnBase: React.CSSProperties = { padding: '5px 12px', borderRadius: 6, border: 'none', cursor: 'pointer', fontSize: 12, fontWeight: 600 }
const btnS: React.CSSProperties = { ...btnBase, background: '#22c55e', color: '#000' }
const btnO: React.CSSProperties = { ...btnBase, background: '#f59e0b', color: '#000' }
const btnR: React.CSSProperties = { ...btnBase, background: '#ef4444', color: '#fff' }
const btnG: React.CSSProperties = { ...btnBase, background: '#374151', color: '#e1e4e8' }
const btnT: React.CSSProperties = { padding: '3px 8px', borderRadius: 4, border: '1px solid #333', background: '#1f2937', color: '#94a3b8', cursor: 'pointer', fontSize: 11 }

