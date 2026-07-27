import { HandoffWindowQueue } from './handoffQueue'

export interface CoreConfig {
  port: number
  credential: string
}

let runtimeConfig: CoreConfig | null = null

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown
  }
}

export function isTauriDesktop(): boolean {
  return Boolean(window.__TAURI_INTERNALS__)
}

export function coreOrigin(): string {
  const port = runtimeConfig?.port || 8765
  return isTauriDesktop() ? `http://127.0.0.1:${port}` : ''
}

export function internalCredential(): string {
  return runtimeConfig?.credential || ''
}

export async function prepareTauriRuntime(): Promise<void> {
  if (!isTauriDesktop()) return
  const { invoke } = await import('@tauri-apps/api/core')
  runtimeConfig = await invoke<CoreConfig>('get_core_config')
}

function apiHeaders(): Record<string, string> {
  return {
    'Content-Type': 'application/json',
    'X-Token': internalCredential(),
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
  const handoffQueue = new HandoffWindowQueue()
  let activeHandoffWindow: InstanceType<typeof WebviewWindow> | null = null
  let openingHandoff = false

  const showMain = async () => {
    await current.show().catch(() => {})
    await current.unminimize().catch(() => {})
    await current.setFocus().catch(() => {})
  }

  const openNextHandoff = async (): Promise<void> => {
    if (stopped || openingHandoff || activeHandoffWindow) return
    const id = handoffQueue.begin()
    if (!id) return
    openingHandoff = true
    const label = `handoff-${id.replace(/[^a-zA-Z0-9-]/g, '-')}`
    try {
      const existing = await WebviewWindow.getByLabel(label)
      const child = existing || new WebviewWindow(label, {
        url: `index.html?handoff=${encodeURIComponent(id)}`,
        title: '下载文件信息 - HLS Downloader',
        width: 420,
        height: 460,
        minWidth: 380,
        minHeight: 360,
        center: true,
        resizable: true,
        decorations: false,
        alwaysOnTop: true,
        focus: true,
      })
      if (!existing) {
        await new Promise<void>((resolve, reject) => {
          void child.once('tauri://created', () => resolve())
          void child.once('tauri://error', event => reject(new Error(String(event.payload || '无法创建下载确认窗口'))))
        })
      }
      activeHandoffWindow = child
      await child.show().catch(() => {})
      await child.setFocus().catch(() => {})
      await localRequest(`/desktop/handoffs/${encodeURIComponent(id)}/presented`, { method: 'POST', body: '{}' })
      void child.once('tauri://destroyed', () => {
        if (handoffQueue.release(id)) activeHandoffWindow = null
        void openNextHandoff()
      })
    } catch {
      handoffQueue.release(id)
      activeHandoffWindow = null
      await localRequest(`/browser/handoffs/${encodeURIComponent(id)}/cancel`, { method: 'POST', body: '{}' }).catch(() => {})
    } finally {
      openingHandoff = false
      void openNextHandoff()
    }
  }

  const openHandoff = async (id: string) => {
    if (!id) return
    if (id === handoffQueue.activeId) {
      await activeHandoffWindow?.show().catch(() => {})
      await activeHandoffWindow?.setFocus().catch(() => {})
      return
    }
    if (handoffQueue.enqueue(id)) await openNextHandoff()
  }

  const poll = async () => {
    while (!stopped) {
      try {
        const result = await localRequest(`/desktop/session/commands?after=${sequence}&timeout=20`)
        const commands = result.commands || []
        for (const command of commands) {
          if (command.kind === 'activate') await showMain()
          else if (command.kind === 'handoff') await openHandoff(String(command.handoff_id || ''))
          else if (command.kind === 'media_push') {
            await showMain()
            const item = await localRequest(`/browser/media-push/${encodeURIComponent(String(command.handoff_id || ''))}`)
            window.dispatchEvent(new CustomEvent('hls-browser-media-push', { detail: item }))
          }
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
