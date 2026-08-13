export const CONNECTION_MAP_LIMIT = 64

export type ConnectionPartState = 'done' | 'active' | 'queued'

export type ConnectionPart = {
  start: number
  end: number
  done: number
  state: ConnectionPartState
}

export type ConnectionMapModel = {
  parts: Array<ConnectionPart & { size: number; fill: number; flex: number }>
  active: number
  doneBytes: number
  total: number
}

function asInt(value: unknown, fallback = 0): number {
  const number = Math.round(Number(value))
  return Number.isFinite(number) ? number : fallback
}

export function normalizeConnectionParts(values: unknown, total = 0): ConnectionPart[] {
  if (!Array.isArray(values)) return []
  const limit = Math.max(0, asInt(total))
  const parts: ConnectionPart[] = []
  for (const item of values.slice(0, CONNECTION_MAP_LIMIT * 4)) {
    if (!item || typeof item !== 'object') continue
    const row = item as Record<string, unknown>
    const rawStart = Number(row.start)
    if (!Number.isFinite(rawStart)) continue
    const start = Math.max(0, Math.round(rawStart))
    let end = Math.max(start, asInt(row.end, start))
    if (limit && start >= limit) continue
    if (limit) end = Math.min(end, limit - 1)
    const size = end - start + 1
    if (size <= 0) continue
    const done = Math.min(size, Math.max(0, asInt(row.done)))
    const raw = String(row.state || '').trim().toLowerCase()
    const state: ConnectionPartState = raw === 'done' || raw === 'active' || raw === 'queued'
      ? raw
      : done >= size ? 'done' : done > 0 ? 'active' : 'queued'
    parts.push({ start, end, done, state })
  }
  parts.sort((a, b) => a.start - b.start || a.end - b.end)
  return parts.slice(0, CONNECTION_MAP_LIMIT)
}

export function buildConnectionMap(values: unknown, total = 0): ConnectionMapModel | null {
  const parts = normalizeConnectionParts(values, total)
  if (!parts.length) return null
  const fileSize = Math.max(asInt(total), parts[parts.length - 1].end + 1)
  let doneBytes = 0
  let active = 0
  const painted = parts.map((part) => {
    const size = part.end - part.start + 1
    doneBytes += Math.min(size, part.done)
    if (part.state === 'active') active += 1
    return { ...part, size, fill: size ? Math.min(100, (part.done * 100) / size) : 0, flex: size }
  })
  return { parts: painted, active, doneBytes, total: fileSize }
}
