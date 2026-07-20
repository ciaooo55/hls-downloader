export const PLAYBACK_RATES = [0.5, 0.75, 1, 1.25, 1.5, 2, 2.5, 3] as const

export function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value))
}

export function timelineTime(
  clientX: number,
  left: number,
  width: number,
  duration: number,
): number {
  if (!Number.isFinite(duration) || duration <= 0 || width <= 0) return 0
  return clamp((clientX - left) / width, 0, 1) * duration
}

export function effectivePlaybackDuration(
  mode: 'hls' | 'file',
  backendTotal: number,
  taskTotal: number,
  availableDuration: number,
  playableDuration: number,
  mediaDuration: number,
): number {
  const finitePositive = (value: number) => Number.isFinite(value) && value > 0 ? value : 0
  if (mode === 'file') {
    return Math.max(finitePositive(mediaDuration), finitePositive(taskTotal))
  }

  // The browser can expose a live-edge duration for an incomplete HLS manifest.
  // Prefer the duration calculated from the original VOD playlist when available.
  const authoritative = finitePositive(backendTotal) || finitePositive(taskTotal)
  if (authoritative > 0) return authoritative
  return Math.max(
    finitePositive(availableDuration),
    finitePositive(playableDuration),
    finitePositive(mediaDuration),
  )
}

export function thumbnailBucket(time: number, duration: number, interval = 10): number {
  if (!Number.isFinite(time) || !Number.isFinite(duration) || duration <= 0) return 0
  const bounded = clamp(time, 0, Math.max(0, duration - 0.05))
  return Math.min(Math.floor(bounded / interval) * interval, bounded)
}

export function thumbnailLeft(
  time: number,
  duration: number,
  trackWidth: number,
  previewWidth = 184,
): number {
  if (trackWidth <= 0 || duration <= 0) return previewWidth / 2
  const raw = (clamp(time / duration, 0, 1) * trackWidth)
  return clamp(raw, previewWidth / 2, Math.max(previewWidth / 2, trackWidth - previewWidth / 2))
}

export function isTimeSeekable(
  ranges: Pick<TimeRanges, 'length' | 'start' | 'end'>,
  time: number,
  tolerance = 0.25,
): boolean {
  if (!Number.isFinite(time) || time < 0) return false
  for (let index = 0; index < ranges.length; index += 1) {
    if (time >= ranges.start(index) - tolerance && time <= ranges.end(index) + tolerance) {
      return true
    }
  }
  return false
}

export function formatPlayerTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00'
  const whole = Math.floor(seconds)
  const hour = Math.floor(whole / 3600)
  const minute = Math.floor((whole % 3600) / 60)
  const second = whole % 60
  return hour > 0
    ? `${hour}:${String(minute).padStart(2, '0')}:${String(second).padStart(2, '0')}`
    : `${minute}:${String(second).padStart(2, '0')}`
}
