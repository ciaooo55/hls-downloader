export const SITE_PROFILE_LIMIT = 100

export type SiteProfile = {
  host: string
  enabled?: boolean
  user_agent?: string
  referer?: string
  origin?: string
  cookie?: string
  download_dir?: string
  request_headers?: Record<string, string>
  concurrency?: number
  speed_limit_kib?: number
  proxy_mode?: '' | 'direct' | 'system' | 'manual'
  proxy_url?: string
}

export function emptySiteProfile(): SiteProfile {
  return {
    host: '',
    enabled: true,
    user_agent: '',
    referer: '',
    origin: '',
    cookie: '',
    download_dir: '',
    request_headers: {},
    concurrency: 0,
    speed_limit_kib: 0,
    proxy_mode: '',
    proxy_url: '',
  }
}

function asInt(value: unknown, min: number, max: number): number {
  const number = Math.round(Number(value) || 0)
  if (!Number.isFinite(number)) return min
  return Math.min(max, Math.max(min, number))
}

export function linesToHeaders(text: string): Record<string, string> {
  const headers: Record<string, string> = {}
  for (const raw of String(text || '').split(/\r?\n/)) {
    const line = raw.trim()
    if (!line) continue
    const index = line.indexOf(':')
    if (index <= 0) continue
    const name = line.slice(0, index).trim()
    const value = line.slice(index + 1).trim()
    if (name) headers[name] = value
  }
  return headers
}

export function headersToLines(headers?: Record<string, string> | null): string {
  if (!headers || typeof headers !== 'object') return ''
  return Object.entries(headers)
    .filter(([name]) => String(name || '').trim())
    .map(([name, value]) => `${name}: ${value ?? ''}`)
    .join('\n')
}

export function normalizeSiteProfiles(values: unknown): SiteProfile[] {
  if (!Array.isArray(values)) return []
  const profiles: SiteProfile[] = []
  for (const item of values.slice(0, SITE_PROFILE_LIMIT)) {
    if (!item || typeof item !== 'object') continue
    const row = item as Record<string, unknown>
    const host = String(row.host || row.pattern || '').trim()
    if (!host) continue
    const headers = row.request_headers && typeof row.request_headers === 'object' && !Array.isArray(row.request_headers)
      ? Object.fromEntries(Object.entries(row.request_headers as Record<string, unknown>).map(([name, value]) => [String(name), String(value ?? '')]))
      : {}
    profiles.push({
      host: host.slice(0, 255),
      enabled: row.enabled !== false,
      user_agent: String(row.user_agent || '').slice(0, 2048),
      referer: String(row.referer || '').slice(0, 4096),
      origin: String(row.origin || '').slice(0, 1024),
      cookie: String(row.cookie || '').slice(0, 16 * 1024),
      download_dir: String(row.download_dir || '').trim().slice(0, 32767),
      request_headers: headers,
      concurrency: asInt(row.concurrency, 0, 64),
      speed_limit_kib: asInt(row.speed_limit_kib, 0, 1048576),
      proxy_mode: ['direct', 'system', 'manual'].includes(String(row.proxy_mode || '')) ? String(row.proxy_mode) as SiteProfile['proxy_mode'] : '',
      proxy_url: ['direct', 'system', 'manual'].includes(String(row.proxy_mode || '')) && String(row.proxy_mode) === 'manual' ? String(row.proxy_url || '').trim().slice(0, 2048) : '',
    })
  }
  return profiles
}

export function moveSiteProfile(profiles: SiteProfile[], from: number, to: number): SiteProfile[] {
  if (from === to || from < 0 || to < 0 || from >= profiles.length || to >= profiles.length) return profiles
  const next = profiles.slice()
  const [item] = next.splice(from, 1)
  next.splice(to, 0, item)
  return next
}
