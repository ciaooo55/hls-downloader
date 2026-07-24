import type { PlaybackSeek, PlaybackSession, PlaybackStatus } from './types'
import { coreOrigin } from './tauri'

const BASE = `${coreOrigin()}/api`

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
  return localStorage.getItem('hls_token') || '55555'
}

function headers(): Record<string, string> {
  return { 'Content-Type': 'application/json', 'X-Token': getToken() }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const requestHeaders = init.body instanceof FormData
    ? { 'X-Token': getToken(), ...(init.headers || {}) }
    : { ...headers(), ...(init.headers || {}) }
  const response = await fetch(`${BASE}${path}`, {
    ...init,
    headers: requestHeaders,
  })
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
export const fetchTorrentFiles = (id: string) =>
  request<{ files: any[]; selected: number[] }>(`/tasks/${id}/files`)
export const selectTorrentFiles = (id: string, indexes: number[]) =>
  request<{ ok: boolean }>(`/tasks/${id}/files`, {
    method: 'PUT',
    body: JSON.stringify({ indexes }),
  })
export const taskAction = (id: string, action: string) =>
  request<{ ok: boolean }>(`/tasks/${id}/${action}`, { method: 'POST' })
export const deleteTask = (id: string, deleteFiles = false) =>
  request<{ ok: boolean }>(`/tasks/${id}${deleteFiles ? '?delete_files=true' : ''}`, { method: 'DELETE' })
export const taskFileUrl = (id: string) =>
  `${BASE}/tasks/${encodeURIComponent(id)}/file?token=${encodeURIComponent(getToken())}`
export const clearCompletedTasks = () =>
  request<{ ok: boolean; count: number }>('/tasks/completed', { method: 'DELETE' })
export const fetchLog = (id: string) => request<{ log: string }>(`/tasks/${id}/log`)
export const openExplorer = (path: string) =>
  request<{ ok: boolean }>('/open-explorer', {
    method: 'POST',
    body: JSON.stringify({ path }),
  })
export const launchFile = (path: string) =>
  request<{ ok: boolean }>('/launch-file', {
    method: 'POST',
    body: JSON.stringify({ path }),
  })
export const browseDir = (path: string = '') =>
  request<any>(`/browse-dir?path=${encodeURIComponent(path)}`)
export const testConnection = () => request<any>('/test')
export const scanTvboxDevices = () => request<{ devices: Array<{ endpoint: string; host: string; port: number; label: string; matched: boolean }> }>('/tvbox/scan')
export const recognizeUrl = (data: any) => request<any>('/recognize', { method: 'POST', body: JSON.stringify(data) })
export const fetchBrowserHandoffs = () => request<any[]>('/browser/handoffs')
export const fetchBrowserHandoff = (id: string) => request<any>(`/browser/handoffs/${encodeURIComponent(id)}`)
export const fetchBrowserStatus = () => request<any>('/browser/status')
export const resolveBrowserHandoff = (id: string, action: 'accept' | 'reject' | 'cancel', data?: object) =>
  request<any>(`/browser/handoffs/${encodeURIComponent(id)}/${action}`, {
    method: 'POST',
    ...(data ? { body: JSON.stringify(data) } : {}),
  })
export const fetchUpdateInfo = (force = false) =>
  request<any>(`/update/check${force ? '?force=true' : ''}`)
export const installUpdate = () =>
  request<{ ok: boolean; version: string }>('/update/install', { method: 'POST' })
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
export const playbackPlaylistUrl = (id: string, session: string, full = true) =>
  `${BASE}/tasks/${encodeURIComponent(id)}/playback/index.m3u8?session=${encodeURIComponent(session)}&token=${encodeURIComponent(getToken())}${full ? '&full=1' : ''}`
export const playbackMediaUrl = (id: string, session: string) =>
  `${BASE}/tasks/${encodeURIComponent(id)}/playback/media?session=${encodeURIComponent(session)}&token=${encodeURIComponent(getToken())}`

export function connectSSE(
  onEvent: (event: any) => void,
  onOpen?: () => void,
): { close: () => void } {
  const token = getToken()
  let closed = false
  let eventSource: EventSource | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null

  function connect() {
    if (closed) return
    eventSource = new EventSource(`${BASE}/events?token=${encodeURIComponent(token)}`)
    eventSource.onopen = () => onOpen?.()
    eventSource.onmessage = message => {
      try { onEvent(JSON.parse(message.data)) } catch {}
    }
    eventSource.onerror = () => {
      if (closed) return
      eventSource?.close()
      reconnectTimer = setTimeout(connect, 3000)
    }
  }

  connect()
  return {
    close() {
      closed = true
      if (reconnectTimer) clearTimeout(reconnectTimer)
      eventSource?.close()
    },
  }
}
