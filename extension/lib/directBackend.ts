export interface DirectBackendIdentity {
  version: string
  client_id: string
  browser: string
}

export const V7_CORE_PROTOCOL = 'hls-downloader-v7-core'
export const V6_CORE_PROTOCOL = 'hls-downloader-v6-core'

export function shouldClearLoopbackBridge(response: {
  protocol?: unknown
  bridge_base?: unknown
  bridge_token?: unknown
} | null | undefined): boolean {
  const protocol = String(response?.protocol || '')
  return protocol === V7_CORE_PROTOCOL || protocol === V6_CORE_PROTOCOL
}

export function shouldAttachLoopbackBridge(response: {
  protocol?: unknown
  bridge_base?: unknown
  bridge_token?: unknown
} | null | undefined): boolean {
  if (shouldClearLoopbackBridge(response)) return false
  if (!response) return false
  return Boolean(response.bridge_base && response.bridge_token)
}

// Ops that must never route through the loopback bridge: takeover decisions and
// secret-bearing calls. BrowserDirectBackend.request deliberately has no branch
// for accept_handoff/reject_handoff — removing an op here without adding an
// HTTP branch is not safe.
const NATIVE_ONLY_OPS: ReadonlySet<string> = new Set([
  'ping',
  'offer',
  'download',
  'handoff_status',
  'wait_handoff',
  'accept_handoff',
  'reject_handoff',
  'set_takeover_settings',
  'media_push',
  'push_to_tv',
  'media_push_status',
])

export function shouldRouteThroughLoopbackBridge(op: unknown, hasLoopback: boolean): boolean {
  if (!hasLoopback) return false
  const operation = String(op || '')
  // Takeover and secret-bearing ops must stay on Native Messaging so a stale
  // 5.x FastAPI pairing cannot receive cookies after v6 Core is running.
  return !NATIVE_ONLY_OPS.has(operation)
}

export class BrowserDirectBackend {
  constructor(private readonly base: string, private readonly token: string) {}

  private async call(path: string, init: RequestInit, timeoutMs: number): Promise<any> {
    const response = await fetch(`${this.base}${path}`, {
      ...init,
      signal: AbortSignal.timeout(timeoutMs),
      headers: {
        'Content-Type': 'application/json',
        'X-Token': this.token,
        ...(init.headers || {}),
      },
    })
    const body = await response.json().catch(() => ({}))
    if (!response.ok) {
      const detail = body?.detail
      throw new Error(typeof detail === 'string' ? detail : `Desktop HTTP ${response.status}`)
    }
    return body
  }

  async request(message: Record<string, any>, identity: DirectBackendIdentity, timeoutMs = 4_000): Promise<any> {
    const op = String(message.op || '')
    const post = (path: string, body: unknown) => this.call(path, { method: 'POST', body: JSON.stringify(body) }, timeoutMs)
    const get = (path: string) => this.call(path, { method: 'GET' }, timeoutMs)
    if (op === 'ping') {
      const browserStatus = await post('/browser/ping', identity)
      return {
        ok: true,
        version: browserStatus.core_version || '',
        takeover_enabled: browserStatus.takeover_enabled !== false,
        takeover_minimum_bytes: Math.max(0, Number(browserStatus.takeover_minimum_bytes || 0)),
        recommended_extension_version: browserStatus.recommended_version || '',
        minimum_extension_version: browserStatus.minimum_version || '',
        extension_release_url: browserStatus.release_url || '',
      }
    }
    if (op === 'offer') return { ok: true, handoff: await post('/browser/handoffs', message.resource || {}) }
    if (op === 'download') return { ok: true, task: await post('/browser/downloads', message.resource || {}), activated: false }
    if (op === 'handoff_status') return { ok: true, handoff: await get(`/browser/handoffs/${encodeURIComponent(message.handoff_id || '')}`) }
    if (op === 'wait_handoff') return { ok: true, handoff: await get(`/browser/handoffs/${encodeURIComponent(message.handoff_id || '')}/wait`) }
    if (op === 'activate') return { ok: true, result: await post('/browser/activate', {}) }
    if (op === 'set_takeover_settings') {
      const payload: Record<string, unknown> = {}
      if ('enabled' in message) payload.browser_takeover_enabled = Boolean(message.enabled)
      if ('minimum_bytes' in message) payload.browser_takeover_min_mb = Math.max(0, Math.floor(Number(message.minimum_bytes || 0) / (1024 * 1024)))
      const current = await post('/browser/takeover-settings', {
        enabled: payload.browser_takeover_enabled,
        minimum_bytes: message.minimum_bytes,
      })
      return {
        ok: true,
        takeover_enabled: current.takeover_enabled !== false,
        takeover_minimum_bytes: Math.max(0, Number(current.takeover_minimum_bytes || 0)),
      }
    }
    if (op === 'push_to_tv') return post('/browser/media-push', { kind: 'tvbox', resource: message.resource || { url: String(message.resource?.url || '') } })
    if (op === 'media_push') return post('/browser/media-push', { kind: String(message.kind || ''), resource: message.resource || {} })
    if (op === 'media_push_status') return get(`/browser/media-push/${encodeURIComponent(message.request_id || '')}/status`)
    throw new Error(`Direct backend does not support ${op}`)
  }
}
