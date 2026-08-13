import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import {
  ChevronDown,
  ChevronUp,
  FastForward,
  GripHorizontal,
  Pause,
  Play,
  Rewind,
  Square,
} from 'lucide-react'
import {
  canControlTransport,
  clampHudPosition,
  clampSeekSeconds,
  downloadControls,
  downloadPercent,
  emptyCastPlayback,
  livePlaybackPosition,
  playbackPercent,
  shareKindLabel,
  shareStopLabel,
  type CastPlaybackStatus,
  type LocalShareSession,
} from '../castSession'
import { pauseLabelFor } from '../taskCommands'
import { fmtBytes, fmtClock } from '../format'
import type { Task } from '../types'

const HUD_BOTTOM_INSET = 44

export default function CastSessionHud({
  share,
  task,
  playback,
  busy,
  minimized,
  onMinimize,
  onRestore,
  onControl,
  onSeekTo,
  onStop,
  onPauseDownload,
  onResumeDownload,
}: {
  share: LocalShareSession
  task?: Task | null
  playback: CastPlaybackStatus
  busy: boolean
  minimized: boolean
  onMinimize: () => void
  onRestore: () => void
  onControl: (action: 'play' | 'pause' | 'seek', seconds?: number) => void
  onSeekTo: (seconds: number) => void
  onStop: () => void
  onPauseDownload: () => void
  onResumeDownload: () => void
}) {
  const panelRef = useRef<HTMLElement | null>(null)
  const dragRef = useRef<{ pointer: number; startX: number; startY: number; left: number; top: number } | null>(null)
  const pendingSeekRef = useRef<number | null>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  const [scrubbing, setScrubbing] = useState<number | null>(null)
  const [clock, setClock] = useState(() => Date.now())
  const sampledAtRef = useRef(Date.now())
  const lastSampleRef = useRef(`${playback.playing}:${playback.paused}:${playback.position}`)
  const sampleKey = `${playback.playing}:${playback.paused}:${playback.position}`
  if (sampleKey !== lastSampleRef.current) {
    lastSampleRef.current = sampleKey
    sampledAtRef.current = Date.now()
  }
  const status = playback.playing || playback.paused ? playback : { ...emptyCastPlayback(), ...playback }
  const duration = Math.max(0, status.duration || 0)
  const position = livePlaybackPosition(status, sampledAtRef.current, clock, scrubbing)
  const transport = canControlTransport(share.kind)
  const downloads = downloadControls(task)
  const deviceLabel = (share.device as { label?: string } | undefined)?.label || status.label || (share.kind === 'cast' ? '电视' : 'TVBox')
  const playing = transport && status.playing && !status.paused
  const downloadPauseLabel = task ? pauseLabelFor([task]) : '暂停下载'
  const stopLabel = shareStopLabel(share)

  useEffect(() => {
    if (!playing || scrubbing != null) return
    const timer = window.setInterval(() => setClock(Date.now()), 250)
    return () => window.clearInterval(timer)
  }, [playing, scrubbing])

  useEffect(() => {
    const onResize = () => {
      if (!pos || !panelRef.current) return
      const box = panelRef.current.getBoundingClientRect()
      const next = clampHudPosition(pos.left, pos.top, box.width, box.height, window.innerWidth, window.innerHeight, 12, HUD_BOTTOM_INSET)
      if (next.left !== pos.left || next.top !== pos.top) setPos(next)
    }
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [pos, minimized])

  useEffect(() => {
    if (!pos || !panelRef.current) return
    const box = panelRef.current.getBoundingClientRect()
    const next = clampHudPosition(pos.left, pos.top, box.width, box.height, window.innerWidth, window.innerHeight, 12, HUD_BOTTOM_INSET)
    if (next.left !== pos.left || next.top !== pos.top) setPos(next)
  }, [minimized, pos])

  useEffect(() => {
    if (busy || pendingSeekRef.current == null) return
    const seconds = pendingSeekRef.current
    pendingSeekRef.current = null
    onSeekTo(seconds)
    setScrubbing(null)
  }, [busy, onSeekTo])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || minimized) return
      if (document.querySelector('.modal-overlay')) return
      event.preventDefault()
      onMinimize()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [minimized, onMinimize])

  const beginDrag = (event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) return
    const target = event.target as HTMLElement
    if (target.closest('button, input, [data-no-drag]')) return
    const box = panelRef.current?.getBoundingClientRect()
    if (!box) return
    event.currentTarget.setPointerCapture(event.pointerId)
    dragRef.current = {
      pointer: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      left: box.left,
      top: box.top,
    }
    setPos({ left: box.left, top: box.top })
  }

  const moveDrag = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointer !== event.pointerId) return
    const box = panelRef.current?.getBoundingClientRect()
    const next = clampHudPosition(
      drag.left + event.clientX - drag.startX,
      drag.top + event.clientY - drag.startY,
      box?.width || 320,
      box?.height || 88,
      window.innerWidth,
      window.innerHeight,
      12,
      HUD_BOTTOM_INSET,
    )
    setPos(next)
  }

  const endDrag = (event: ReactPointerEvent<HTMLElement>) => {
    if (dragRef.current?.pointer === event.pointerId) dragRef.current = null
  }

  const seekFromBar = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!transport || !duration) return
    const rect = event.currentTarget.getBoundingClientRect()
    const ratio = Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width))
    const seconds = clampSeekSeconds(ratio * duration, duration)
    setScrubbing(seconds)
  }

  const flushSeek = (seconds: number) => {
    if (busy) {
      pendingSeekRef.current = seconds
      return
    }
    pendingSeekRef.current = null
    onSeekTo(seconds)
    setScrubbing(null)
  }

  const commitSeek = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (scrubbing == null) return
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
    flushSeek(scrubbing)
  }

  return (
    <aside
      ref={panelRef}
      className={`cast-session-hud${minimized ? ' is-min' : ''}${playing ? ' is-live' : ''}${busy ? ' is-busy' : ''}`}
      style={pos ? { left: pos.left, top: pos.top, right: 'auto', bottom: 'auto' } : undefined}
      role="complementary"
      aria-label={`${shareKindLabel(share.kind)}控制`}
      data-kind={share.kind}
    >
      <header className="cast-hud-head" onPointerDown={beginDrag} onPointerMove={moveDrag} onPointerUp={endDrag} onPointerCancel={endDrag}>
        <GripHorizontal size={14} className="cast-hud-grip" aria-hidden />
        <span className={`cast-hud-live${playing ? ' on' : ''}`} aria-hidden />
        <div className="cast-hud-titles">
          <strong>{shareKindLabel(share.kind)} · {deviceLabel}</strong>
          <em title={share.filename}>{share.filename}</em>
        </div>
        <button type="button" className="cast-hud-icon" title={minimized ? '展开控制面板' : '收起为悬浮条'} aria-label={minimized ? '展开' : '收起'} onClick={minimized ? onRestore : onMinimize}>
          {minimized ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
        </button>
        <button type="button" className="cast-hud-icon danger" title={stopLabel} aria-label={stopLabel} disabled={busy} onClick={onStop}>
          <Square size={13} />
        </button>
      </header>

      {minimized ? (
        <div className="cast-hud-mini" data-no-drag>
          {transport && (
            <button type="button" disabled={busy} title={playing ? '暂停投屏' : '继续投屏'} aria-label={playing ? '暂停' : '播放'} onClick={() => onControl(playing ? 'pause' : 'play')}>
              {playing ? <Pause size={15} /> : <Play size={15} />}
            </button>
          )}
          {downloads.pause && (
            <button type="button" disabled={busy} title={downloadPauseLabel} aria-label={downloadPauseLabel} onClick={onPauseDownload}>
              <Pause size={15} />
            </button>
          )}
          {downloads.resume && (
            <button type="button" disabled={busy} title="恢复下载" aria-label="恢复下载" onClick={onResumeDownload}>
              <Play size={15} />
            </button>
          )}
          <button type="button" className="cast-hud-restore" onClick={onRestore}>{transport ? fmtClock(position) : '共享中'}</button>
        </div>
      ) : (
        <div className="cast-hud-body">
          {transport ? (
            <>
              <div className="cast-hud-transport" data-no-drag>
                <button type="button" disabled={busy} title="后退 10 秒" aria-label="后退 10 秒" onClick={() => onControl('seek', -10)}><Rewind size={16} /></button>
                <button type="button" className="cast-hud-play" disabled={busy} title={playing ? '暂停投屏播放' : '继续投屏播放'} aria-label={playing ? '暂停' : '播放'} onClick={() => onControl(playing ? 'pause' : 'play')}>
                  {playing ? <Pause size={18} /> : <Play size={18} />}
                </button>
                <button type="button" disabled={busy} title="快进 10 秒" aria-label="快进 10 秒" onClick={() => onControl('seek', 10)}><FastForward size={16} /></button>
                <span className="cast-hud-times">
                  <b>{fmtClock(position)}</b>
                  <i>/</i>
                  <em>{duration > 0 ? fmtClock(duration) : '--:--'}</em>
                </span>
              </div>
              <div
                className="cast-hud-scrub"
                data-no-drag
                role="slider"
                aria-label="投屏进度"
                aria-valuemin={0}
                aria-valuemax={Math.max(1, duration)}
                aria-valuenow={Math.floor(position)}
                aria-valuetext={`${fmtClock(position)} / ${duration > 0 ? fmtClock(duration) : '--:--'}`}
                tabIndex={0}
                onPointerDown={event => {
                  event.currentTarget.setPointerCapture(event.pointerId)
                  seekFromBar(event)
                }}
                onPointerMove={event => { if (scrubbing != null) seekFromBar(event) }}
                onPointerUp={commitSeek}
                onPointerCancel={commitSeek}
                onKeyDown={event => {
                  if (event.key === 'ArrowLeft') { event.preventDefault(); onControl('seek', -10) }
                  if (event.key === 'ArrowRight') { event.preventDefault(); onControl('seek', 10) }
                  if (event.key === 'Home') { event.preventDefault(); onSeekTo(0) }
                  if (event.key === 'End' && duration) { event.preventDefault(); onSeekTo(duration) }
                  if (event.key === ' ') { event.preventDefault(); onControl(playing ? 'pause' : 'play') }
                }}
              >
                <i style={{ transform: `scaleX(${playbackPercent(position, duration) / 100})` }} />
              </div>
            </>
          ) : (
            <p className="cast-hud-note">TVBox 没有通用远程播放协议，进度与暂停在电视端操作。本机可随时停止共享{task ? '，也可以暂停这条下载' : ''}。</p>
          )}

          {task && (
            <div className="cast-hud-download" data-no-drag>
              <div className="cast-hud-download-meta">
                <span>下载{task.status === 'done' ? '已完成' : task.status === 'paused' ? '已暂停' : '进行中'}</span>
                <em>{fmtBytes(task.downloaded_bytes)}{task.total_bytes > 0 ? ` / ${fmtBytes(task.total_bytes)}` : ''}</em>
              </div>
              <div className="cast-hud-download-bar" aria-hidden>
                <i style={{ transform: `scaleX(${downloadPercent(task) / 100})` }} />
              </div>
              <div className="cast-hud-download-actions">
                {downloads.pause && <button type="button" disabled={busy} onClick={onPauseDownload}><Pause size={13} />{downloadPauseLabel}</button>}
                {downloads.resume && <button type="button" disabled={busy} onClick={onResumeDownload}><Play size={13} />恢复下载</button>}
                {!downloads.pause && !downloads.resume && <small>{task.status === 'done' ? '文件已可边播边看' : '当前阶段不能暂停下载'}</small>}
              </div>
            </div>
          )}
        </div>
      )}
    </aside>
  )
}
