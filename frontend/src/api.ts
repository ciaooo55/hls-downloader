import type { PlaybackSeek, PlaybackSession, PlaybackStatus } from './types'
import { coreOrigin, internalCredential, prepareTauriRuntime } from './tauri'

// The Tauri core port is loaded asynchronously from the runtime config.  Do
// not capture coreOrigin() during module evaluation: that happens before
// prepareTauriRuntime() and silently pins a custom-port installation to 8765.
function apiBase(): string {
  return `${coreOrigin()}/api`
}

export class ApiError extends Error {
  status: number
  detail: unknown

  constructor(status: number, message: string, detail: unknown = null) {
    super(message)
    this.status = status
    this.detail = detail
  }
}

export function isDuplicateUrlError(error: unknown): error is ApiError {
  if (!(error instanceof ApiError) || error.status !== 409) return false
  const detail = error.detail
  if (detail && typeof detail === 'object' && (detail as { code?: string }).code === 'DUPLICATE_URL') return true
  return typeof error.message === 'string' && error.message.includes('相同链接')
}

export function getToken(): string {
  return internalCredential()
}

function headers(): Record<string, string> {
  return { 'Content-Type': 'application/json', 'X-Token': getToken() }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const requestHeaders = init.body instanceof FormData
    ? { 'X-Token': getToken(), ...(init.headers || {}) }
    : { ...headers(), ...(init.headers || {}) }
  let response = await fetch(`${apiBase()}${path}`, {
    ...init,
    headers: requestHeaders,
  })
  if (response.status === 401) {
    await prepareTauriRuntime(true)
    const refreshedHeaders = init.body instanceof FormData
      ? { 'X-Token': getToken(), ...(init.headers || {}) }
      : { ...headers(), ...(init.headers || {}) }
    response = await fetch(`${apiBase()}${path}`, { ...init, headers: refreshedHeaders })
  }
  const body = await response.json().catch(() => ({} as any))
  if (!response.ok) {
    const detail = body?.detail
    let message = `HTTP ${response.status}`
    if (typeof detail === 'string') message = detail
    else if (detail && typeof detail === 'object' && typeof (detail as any).message === 'string') message = (detail as any).message
    else if (Array.isArray(detail) && detail[0]?.msg) message = detail.map((item: any) => item.msg).join('; ')
    throw new ApiError(response.status, message, detail ?? body)
  }
  return body as T
}

export const fetchSettings = () => request<any>('/settings')
export const fetchHealth = () => request<{ status: string; version: string }>('/health')
export const saveSettings = (data: any) =>
  request<any>('/settings', { method: 'POST', body: JSON.stringify(data) })
export const fetchTasks = () => request<any[]>('/tasks')
export const createTask = (data: any) =>
  request<any>('/tasks', { method: 'POST', body: JSON.stringify(data) })
export const createBatch = (tasks: any[]) =>
  request<any[]>('/tasks/batch', { method: 'POST', body: JSON.stringify({ tasks }) })
export const uploadTorrent = (file: File, title = '') => {
  const body = new FormData()
  body.append('file', file)
  body.append('title', title)
  return request<any>('/tasks/torrent-file', { method: 'POST', body, headers: {} })
}
export const importTorrentPath = (path: string) => request<any>('/tasks/torrent-path', { method: 'POST', body: JSON.stringify({ path }) })
export const fetchTorrentFiles = (id: string) =>
  request<{ files: any[]; selected: number[] }>(`/tasks/${id}/files`)
export const selectTorrentFiles = (id: string, indexes: number[]) =>
  request<{ ok: boolean }>(`/tasks/${id}/files`, {
    method: 'PUT',
    body: JSON.stringify({ indexes }),
  })
export const taskAction = (id: string, action: string) =>
  request<{ ok: boolean }>(`/tasks/${id}/${action}`, { method: 'POST' })
export const refreshTaskRequest = (id: string, data: Record<string, unknown>) =>
  request<any>(`/tasks/${id}/request`, { method: 'PATCH', body: JSON.stringify(data) })
export const setTaskSpeedLimit = (id: string, limitKib: number) =>
  request<{ ok: boolean }>(`/tasks/${id}/speed-limit`, {
    method: 'POST',
    body: JSON.stringify({ limit_kib: limitKib }),
  })
export const deleteTask = (id: string, deleteFiles = false) =>
  request<{ ok: boolean }>(`/tasks/${id}${deleteFiles ? '?delete_files=true' : ''}`, { method: 'DELETE' })
export const taskFileUrl = (id: string, fileAccessToken: string) =>
  `${apiBase()}/tasks/${encodeURIComponent(id)}/file?token=${encodeURIComponent(fileAccessToken)}`
export const clearCompletedTasks = () =>
  request<{ ok: boolean; count: number }>('/tasks/completed', { method: 'DELETE' })
export const fetchLog = (id: string) => request<{ log: string }>(`/tasks/${id}/log`)
export const openExplorer = (path: string) =>
  request<{ ok: boolean }>('/open-explorer', {
    method: 'POST',
    body: JSON.stringify({ path }),
  })
export const openTaskInExplorer = (taskId: string) =>
  request<{ ok: boolean }>('/open-explorer', {
    method: 'POST',
    body: JSON.stringify({ task_id: taskId }),
  })
export const launchFile = (taskId: string, confirmExecutable = false) =>
  request<{ ok: boolean }>('/launch-file', {
    method: 'POST',
    body: JSON.stringify({ task_id: taskId, confirm_executable: confirmExecutable }),
  })
export const browseDir = (path: string = '') =>
  request<any>(`/browse-dir?path=${encodeURIComponent(path)}`)
export const testConnection = () => request<any>('/test')
export const scanTvboxDevices = () => request<{ devices: Array<{ endpoint: string; host: string; port: number; label: string; matched: boolean }> }>('/tvbox/scan')
export const scanCastDevices = () => request<{ devices: Array<{ id: string; protocol: 'dlna' | 'chromecast'; location: string; control_url: string; service_type: string; label: string; host: string }> }>('/cast/scan')
export const pushLocalTvboxFile = (path: string, endpoint = '') => request<{ ok: boolean; endpoint: string; share: { id: string; url: string; filename: string; size: number; expires_in_seconds: number; idle_cleanup_seconds: number } }>('/tvbox/push-local', { method: 'POST', body: JSON.stringify({ path, endpoint }) })
export const castLocalFile = (path: string, device?: object) => request<{ ok: boolean; label: string; share: { id: string; url: string; filename: string; size: number; expires_in_seconds: number; idle_cleanup_seconds: number } }>('/cast/push-local', { method: 'POST', body: JSON.stringify({ path, device }) })
export const pushTvboxUrl = (url: string, endpoint: string) => request<{ ok: boolean; endpoint: string }>('/tvbox/push', { method: 'POST', body: JSON.stringify({ url, endpoint }) })
export const castMediaUrl = (url: string, filename: string, device: object) => request<{ ok: boolean; label: string }>('/cast/push', { method: 'POST', body: JSON.stringify({ url, filename, device }) })
export const controlCast = (action: 'play' | 'pause' | 'seek', seconds = 0, device?: object) => request<{ ok: boolean; label: string }>('/cast/control', { method: 'POST', body: JSON.stringify({ action, seconds, device }) })
export const stopLocalTvboxShare = (shareId: string) => request<{ ok: boolean }>(`/tvbox/shares/${encodeURIComponent(shareId)}/stop`, { method: 'POST' })
export const fetchLocalTvboxShare = (shareId: string) => request<{ active: boolean; filename?: string; active_streams?: number; expires_in_seconds?: number }>(`/tvbox/shares/${encodeURIComponent(shareId)}`)
export const recognizeUrl = (data: any) => request<any>('/recognize', { method: 'POST', body: JSON.stringify(data) })
export interface ManifestTrackOption { id: string; width?: number; height?: number; bandwidth?: number; codecs?: string; lang?: string }
export const fetchManifestTracks = (data: any) =>
  request<{ format: string; video: ManifestTrackOption[]; audio: ManifestTrackOption[] }>('/manifest/tracks', { method: 'POST', body: JSON.stringify(data) })
export const fetchBrowserHandoffs = () => request<any[]>('/browser/handoffs')
export const fetchBrowserHandoff = (id: string) => request<any>(`/browser/handoffs/${encodeURIComponent(id)}`)
export const completeBrowserMediaPush = (id: string, status: 'done' | 'failed' | 'canceled', message = '') => request<{ ok: boolean }>(`/browser/media-push/${encodeURIComponent(id)}/complete`, { method: 'POST', body: JSON.stringify({ status, message }) })
export const fetchBrowserStatus = () => request<any>('/browser/status')
export const resolveBrowserHandoff = (id: string, action: 'accept' | 'reject' | 'cancel', data?: object) =>
  request<any>(`/browser/handoffs/${encodeURIComponent(id)}/${action}`, {
    method: 'POST',
    ...(data ? { body: JSON.stringify(data) } : {}),
  })
export const fetchUpdateInfo = (force = false) =>
  request<any>(`/update/check${force ? '?force=true' : ''}`)
export const installUpdate = () =>
  request<{ ok: boolean; version: string; task_id: string }>('/update/install', { method: 'POST' })
export const cancelPowerAction = (id: string) =>
  request<{ ok: boolean }>(`/power-actions/${encodeURIComponent(id)}/cancel`, { method: 'POST' })
export const confirmPowerAction = (id: string) =>
  request<{ ok: boolean }>(`/power-actions/${encodeURIComponent(id)}/confirm`, { method: 'POST' })
export const fetchPendingPowerActions = () =>
  request<Array<{ power_action_id: string; action: 'shutdown' | 'sleep' | 'hibernate'; task_title: string; delay_seconds: number }>>('/power-actions')
export const createPlaybackSession = (id: string) =>
  request<PlaybackSession>(`/tasks/${id}/playback`, { method: 'POST' })
export const fetchPlaybackStatus = (id: string, session: string) =>
  request<PlaybackStatus>(`/tasks/${id}/playback/status?session=${encodeURIComponent(session)}`)
export const heartbeatPlayback = (id: string, session: string) =>
  request<{ ok: boolean }>(`/tasks/${id}/playback/heartbeat?session=${encodeURIComponent(session)}`, { method: 'POST' })
export const requestPlaybackSeek = (id: string, session: string, time: number) =>
  request<PlaybackSeek>(`/tasks/${id}/playback/seek?session=${encodeURIComponent(session)}`, {
    method: 'POST',
    body: JSON.stringify({ time }),
  })
export const closePlaybackSession = (id: string, session: string) =>
  request<{ ok: boolean }>(`/tasks/${id}/playback?session=${encodeURIComponent(session)}`, { method: 'DELETE', keepalive: true })
export const playbackPlaylistUrl = (id: string, session: string, playbackToken: string, full = true) =>
  `${apiBase()}/tasks/${encodeURIComponent(id)}/playback/index.m3u8?session=${encodeURIComponent(session)}&token=${encodeURIComponent(playbackToken)}${full ? '&full=1' : ''}`
export const playbackMediaUrl = (id: string, session: string, playbackToken: string) =>
  `${apiBase()}/tasks/${encodeURIComponent(id)}/playback/media?session=${encodeURIComponent(session)}&token=${encodeURIComponent(playbackToken)}`

export function connectSSE(
  onEvent: (event: any) => void,
  onOpen?: () => void,
): { close: () => void } {
  let closed = false
  let controller: AbortController | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null

  async function connect() {
    if (closed) return
    controller = new AbortController()
    try {
      const response = await fetch(`${apiBase()}/events`, {
        headers: { 'X-Token': getToken(), Accept: 'text/event-stream' },
        cache: 'no-store',
        signal: controller.signal,
      })
      if (response.status === 401) {
        await prepareTauriRuntime(true)
        if (!closed) reconnectTimer = setTimeout(() => { void connect() }, 0)
        return
      }
      if (!response.ok || !response.body) throw new Error(`SSE HTTP ${response.status}`)
      onOpen?.()
      const reader = response.body.getReader()
      const decoder = new TextDecoder()
      let buffer = ''
      while (!closed) {
        const { value, done } = await reader.read()
        if (done) break
        buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, '\n')
        let boundary = buffer.indexOf('\n\n')
        while (boundary >= 0) {
          const block = buffer.slice(0, boundary)
          buffer = buffer.slice(boundary + 2)
          const data = block.split('\n')
            .filter(line => line.startsWith('data:'))
            .map(line => line.slice(5).trimStart())
            .join('\n')
          if (data) {
            try { onEvent(JSON.parse(data)) } catch {}
          }
          boundary = buffer.indexOf('\n\n')
        }
      }
    } catch {
      if (!closed) reconnectTimer = setTimeout(() => { void connect() }, 3000)
    }
  }

  void connect()
  return {
    close() {
      closed = true
      if (reconnectTimer) clearTimeout(reconnectTimer)
      controller?.abort()
    },
  }
}
