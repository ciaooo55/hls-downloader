export function parseHhmm(value: unknown): [number, number] | null {
  const text = String(value || '').trim()
  const parts = text.split(':')
  if (parts.length < 2) return null
  const hour = Number(parts[0])
  const minute = Number(parts[1])
  if (!Number.isInteger(hour) || !Number.isInteger(minute)) return null
  if (hour < 0 || hour > 23 || minute < 0 || minute > 59) return null
  return [hour, minute]
}

export function insideSpeedWindow(now: Date, start: [number, number], end: [number, number]): boolean {
  const current: [number, number] = [now.getHours(), now.getMinutes()]
  const currentKey = current[0] * 60 + current[1]
  const startKey = start[0] * 60 + start[1]
  const endKey = end[0] * 60 + end[1]
  if (startKey < endKey) return currentKey >= startKey && currentKey < endKey
  return currentKey >= startKey || currentKey < endKey
}

export function effectiveSpeedLimitKib(
  settings: {
    download_speed_limit_kib?: number
    speed_schedule_enabled?: boolean
    speed_schedule_start?: string
    speed_schedule_end?: string
    speed_schedule_limit_kib?: number
  },
  now = new Date(),
): number {
  const base = Math.max(0, Math.min(1048576, Math.round(Number(settings.download_speed_limit_kib) || 0)))
  if (!settings.speed_schedule_enabled) return base
  const start = parseHhmm(settings.speed_schedule_start ?? '08:00')
  const end = parseHhmm(settings.speed_schedule_end ?? '23:00')
  if (!start || !end || (start[0] === end[0] && start[1] === end[1])) return base
  if (!insideSpeedWindow(now, start, end)) return base
  return Math.max(0, Math.min(1048576, Math.round(Number(settings.speed_schedule_limit_kib) || 0)))
}
