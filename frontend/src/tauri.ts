export interface CoreConfig {
  port: number
  token: string
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown
    __HLS_CORE__?: CoreConfig
  }
}

export function isTauriDesktop(): boolean {
  return Boolean(window.__TAURI_INTERNALS__)
}

export function coreOrigin(): string {
  const port = window.__HLS_CORE__?.port || 8765
  return isTauriDesktop() ? `http://127.0.0.1:${port}` : ''
}

export async function prepareTauriRuntime(): Promise<void> {
  if (!isTauriDesktop()) return
  const { invoke } = await import('@tauri-apps/api/core')
  const config = await invoke<CoreConfig>('get_core_config')
  window.__HLS_CORE__ = config
  localStorage.setItem('hls_token', config.token)
}

function apiHeaders(): Record<string, string> {
  return {
    'Content-Type': 'application/json',
    'X-Token': window.__HLS_CORE__?.token || localStorage.getItem('hls_token') || '55555',
  }
}

async function localRequest(path: string, init: RequestInit = {}): Promise<any> {
  const response = await fetch(`${coreOrigin()}/api${path}`, {
    ...init,
    headers: { ...apiHeaders(), ...(init.headers || {}) },
  })
  if (!response.ok) throw new Error(`Desktop bridge HTTP ${response.status}`)
  return response.json()
}

export async function startTauriDesktopSession(): Promise<() => void> {
  if (!isTauriDesktop()) return () => {}
  const [{ WebviewWindow }, { getCurrentWindow }, process] = await Promise.all([
    import('@tauri-apps/api/webviewWindow'),
    import('@tauri-apps/api/window'),
    import('@tauri-apps/plugin-process'),
  ])
  await localRequest('/desktop/session/start', { method: 'POST', body: '{}' })
  const current = getCurrentWindow()
  let stopped = false
  let sequence = 0
  const handoffWindows = new Map<string, InstanceType<typeof WebviewWindow>>()

  const showMain = async () => {
    await current.show().catch(() => {})
    await current.unminimize().catch(() => {})
    await current.setFocus().catch(() => {})
  }

  const openHandoff = async (id: string) => {
    if (!id || handoffWindows.has(id)) return
    const label = `handoff-${id.replace(/[^a-zA-Z0-9-]/g, '-')}`
    const existing = await WebviewWindow.getByLabel(label)
    if (existing) {
      handoffWindows.set(id, existing)
      await existing.show().catch(() => {})
      await existing.setFocus().catch(() => {})
      return
    }
    const token = encodeURIComponent(window.__HLS_CORE__?.token || '55555')
    const child = new WebviewWindow(label, {
      url: `index.html?handoff=${encodeURIComponent(id)}&token=${token}`,
      title: '下载文件信息 - HLS Downloader',
      width: 560,
      height: 680,
      minWidth: 500,
      minHeight: 600,
      center: true,
      resizable: true,
      alwaysOnTop: true,
      focus: true,
    })
    handoffWindows.set(id, child)
    await new Promise<void>((resolve, reject) => {
      void child.once('tauri://created', () => resolve())
      void child.once('tauri://error', event => reject(new Error(String(event.payload || '无法创建下载确认窗口'))))
    })
    await localRequest(`/desktop/handoffs/${encodeURIComponent(id)}/presented`, { method: 'POST', body: '{}' })
    void child.once('tauri://destroyed', () => handoffWindows.delete(id))
  }

  const poll = async () => {
    while (!stopped) {
      try {
        const result = await localRequest(`/desktop/session/commands?after=${sequence}&timeout=20`)
        const commands = result.commands || []
        for (const command of commands) {
          if (command.kind === 'activate') await showMain()
          else if (command.kind === 'handoff') await openHandoff(String(command.handoff_id || ''))
          else if (command.kind === 'shutdown') {
            stopped = true
            await process.exit(0)
          }
          sequence = Math.max(sequence, Number(command.sequence) || 0)
        }
        if (!commands.length) sequence = Math.max(sequence, Number(result.sequence) || 0)
      } catch {
        if (!stopped) await new Promise(resolve => window.setTimeout(resolve, 700))
      }
    }
  }
  void poll()
  return () => {
    stopped = true
    void localRequest('/desktop/session/stop', { method: 'POST', body: '{}' }).catch(() => {})
  }
}
