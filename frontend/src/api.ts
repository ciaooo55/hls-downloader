const BASE = '/api'

export class ApiError extends Error {
  status: number

  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

function getToken(): string {
  return localStorage.getItem('hls_token') || '55555'
}

function headers(): Record<string, string> {
  return { 'Content-Type': 'application/json', 'X-Token': getToken() }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(`${BASE}${path}`, {
    ...init,
    headers: { ...headers(), ...(init.headers || {}) },
  })
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new ApiError(response.status, body.detail || `HTTP ${response.status}`)
  }
  return body as T
}

export const fetchSettings = () => request<any>('/settings')
export const saveSettings = (data: any) =>
  request<{ ok: boolean }>('/settings', { method: 'POST', body: JSON.stringify(data) })
export const fetchTasks = () => request<any[]>('/tasks')
export const createTask = (data: any) =>
  request<any>('/tasks', { method: 'POST', body: JSON.stringify(data) })
export const createBatch = (tasks: any[]) =>
  request<any[]>('/tasks/batch', { method: 'POST', body: JSON.stringify({ tasks }) })
export const taskAction = (id: string, action: string) =>
  request<{ ok: boolean }>(`/tasks/${id}/${action}`, { method: 'POST' })
export const deleteTask = (id: string) =>
  request<{ ok: boolean }>(`/tasks/${id}`, { method: 'DELETE' })
export const fetchLog = (id: string) => request<{ log: string }>(`/tasks/${id}/log`)
export const openExplorer = (path: string) =>
  request<{ ok: boolean }>('/open-explorer', {
    method: 'POST',
    body: JSON.stringify({ path }),
  })
export const browseDir = (path: string = '') =>
  request<any>(`/browse-dir?path=${encodeURIComponent(path)}`)
export const testConnection = () => request<any>('/test')
export const recognizeUrl = (data: any) => request<any>('/recognize', { method: 'POST', body: JSON.stringify(data) })
export const fetchUserscriptStatus = () => request<any>('/userscript/status')
export const fetchUpdateInfo = (force = false) =>
  request<any>(`/update/check${force ? '?force=true' : ''}`)
export const installUpdate = () =>
  request<{ ok: boolean; version: string }>('/update/install', { method: 'POST' })

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
