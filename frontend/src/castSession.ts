import { commandState } from './taskCommands'

export type LocalShareKind = 'cast' | 'tvbox'

export interface LocalShareSession {
  id: string
  filename: string
  idleCleanupSeconds: number
  kind: LocalShareKind
  device?: object
  taskId?: string
}

export interface CastPlaybackStatus {
  ok?: boolean
  label?: string
  playing: boolean
  paused: boolean
  position: number
  duration: number
  state?: string
}

export type CastTransportAction = 'play' | 'pause' | 'seek' | 'seek_to' | 'status'

export function emptyCastPlayback(): CastPlaybackStatus {
  return { playing: false, paused: false, position: 0, duration: 0 }
}

export function mergeCastPlayback(
  current: CastPlaybackStatus,
  incoming: Partial<CastPlaybackStatus> | null | undefined,
): CastPlaybackStatus {
  if (!incoming) return current
  return {
    ok: incoming.ok ?? current.ok,
    label: incoming.label || current.label,
    playing: Boolean(incoming.playing),
    paused: Boolean(incoming.paused),
    position: Math.max(0, Number(incoming.position) || 0),
    duration: Math.max(0, Number(incoming.duration) || 0),
    state: incoming.state || current.state,
  }
}

export function playbackPercent(position: number, duration: number): number {
  if (!duration || duration <= 0) return 0
  return Math.max(0, Math.min(100, (position / duration) * 100))
}

export function clampSeekSeconds(seconds: number, duration: number): number {
  const next = Math.max(0, Math.floor(Number(seconds) || 0))
  if (!duration || duration <= 0) return next
  return Math.min(duration, next)
}

export function livePlaybackPosition(
  playback: CastPlaybackStatus,
  sampledAtMs: number,
  nowMs: number,
  scrubbing: number | null = null,
): number {
  if (scrubbing != null) return scrubbing
  const base = Math.max(0, Number(playback.position) || 0)
  if (!playback.playing || playback.paused) return base
  const elapsed = Math.max(0, (nowMs - sampledAtMs) / 1000)
  return clampSeekSeconds(base + elapsed, playback.duration || 0)
}

export function relativeSeekTarget(position: number, delta: number, duration: number): number {
  return clampSeekSeconds(position + delta, duration)
}

export function downloadPercent(task: { downloaded_bytes: number; total_bytes: number; post_percent?: number } | null | undefined): number {
  if (!task) return 0
  if (task.total_bytes > 0) {
    return Math.max(0, Math.min(100, (task.downloaded_bytes / task.total_bytes) * 100))
  }
  const post = Number(task.post_percent) || 0
  return Math.max(0, Math.min(100, post))
}

export function shareKindLabel(kind: LocalShareKind): string {
  return kind === 'cast' ? '投屏播放' : 'TVBox 推送'
}

export function canControlTransport(kind: LocalShareKind): boolean {
  return kind === 'cast'
}

export function downloadControls(task: { id: string; status: string; available_actions?: string[] } | null | undefined): { pause: boolean; resume: boolean } {
  if (!task) return { pause: false, resume: false }
  const commands = commandState([task])
  return { pause: commands.pause, resume: commands.resume }
}

export function clampHudPosition(
  left: number,
  top: number,
  width: number,
  height: number,
  viewportWidth: number,
  viewportHeight: number,
  margin = 12,
): { left: number; top: number } {
  const maxLeft = Math.max(margin, viewportWidth - width - margin)
  const maxTop = Math.max(margin, viewportHeight - height - margin)
  return {
    left: Math.min(maxLeft, Math.max(margin, left)),
    top: Math.min(maxTop, Math.max(margin, top)),
  }
}
