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

let webCredentialRequest: Promise<void> | null = null

export async function prepareTauriRuntime(force = false): Promise<void> {
  if (isTauriDesktop()) {
    const { invoke } = await import('@tauri-apps/api/core')
    runtimeConfig = await invoke<CoreConfig>('get_core_config')
    return
  }
  if (!force && runtimeConfig?.credential) return
  // A burst of 401s (the task list and SSE reconnecting together) must share
  // one refresh. Otherwise each caller overwrites the in-flight promise and
  // one completion can clear the pointer while another refresh is still
  // running, producing a short-lived "not connected" flap.
  if (webCredentialRequest) return webCredentialRequest
  webCredentialRequest = (async () => {
    let credential = ''
    let resolvedPort = Number(globalThis.location?.port || 8765)
    try {
      credential = globalThis.sessionStorage?.getItem('hls-downloader-ui-credential') || ''
    } catch {
      // Session storage can be disabled; the in-memory credential still works.
    }
    if (!credential || force) {
      const response = await fetch('/api/ui/credential', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        cache: 'no-store',
      })
      const payload = await response.json().catch(() => ({}))
      if (!response.ok || typeof payload?.credential !== 'string' || !payload.credential) {
        throw new Error(typeof payload?.detail === 'string' ? payload.detail : `UI 凭据获取失败（HTTP ${response.status}）`)
      }
      credential = payload.credential
      resolvedPort = Number(payload?.port || resolvedPort)
      try { globalThis.sessionStorage?.setItem('hls-downloader-ui-credential', credential) } catch {}
    }
    runtimeConfig = {
      port: Number.isInteger(resolvedPort) && resolvedPort > 0 && resolvedPort <= 65535
        ? resolvedPort
        : 8765,
      credential,
    }
  })()
  try {
    await webCredentialRequest
  } finally {
    webCredentialRequest = null
  }
}

function apiHeaders(): Record<string, string> {
  return {
    'Content-Type': 'application/json',
    'X-Token': internalCredential(),
  }
}

async function localRequest(path: string, init: RequestInit = {}): Promise<any> {
  let response = await fetch(`${coreOrigin()}/api${path}`, {
    ...init,
    headers: { ...apiHeaders(), ...(init.headers || {}) },
  })
  if (response.status === 401) {
    await prepareTauriRuntime()
    response = await fetch(`${coreOrigin()}/api${path}`, {
      ...init,
      headers: { ...apiHeaders(), ...(init.headers || {}) },
    })
  }
  if (!response.ok) throw new Error(`Desktop bridge HTTP ${response.status}`)
  return response.json()
}

let desktopSessionStart: Promise<() => void> | null = null
let desktopSessionStop: (() => void) | null = null
let desktopSessionReferences = 0

export async function startTauriDesktopSession(): Promise<() => void> {
  if (!isTauriDesktop()) return () => {}
  if (!desktopSessionStart) {
    desktopSessionStart = createTauriDesktopSession()
      .then(stop => {
        desktopSessionStop = stop
        return stop
      })
      .catch(reason => {
        desktopSessionStart = null
        desktopSessionStop = null
        throw reason
      })
  }
  desktopSessionReferences += 1
  try {
    await desktopSessionStart
  } catch (reason) {
    desktopSessionReferences = Math.max(0, desktopSessionReferences - 1)
    throw reason
  }
  let released = false
  return () => {
    if (released) return
    released = true
    desktopSessionReferences = Math.max(0, desktopSessionReferences - 1)
    if (desktopSessionReferences === 0) {
      desktopSessionStop?.()
      desktopSessionStop = null
      desktopSessionStart = null
    }
  }
}

async function createTauriDesktopSession(): Promise<() => void> {
  if (!isTauriDesktop()) return () => {}
  const [{ WebviewWindow }, { getCurrentWindow }, process, { emitTo }] = await Promise.all([
    import('@tauri-apps/api/webviewWindow'),
    import('@tauri-apps/api/window'),
    import('@tauri-apps/plugin-process'),
    import('@tauri-apps/api/event'),
  ])
  await localRequest('/desktop/session/start', { method: 'POST', body: '{}' })
  const current = getCurrentWindow()
  let stopped = false
  let sequence = 0
  const handoffQueue = new HandoffWindowQueue()
  let openingHandoff = false
  let handoffHostReady = false
  let resolveHandoffHostReady: (() => void) | null = null
  const handoffHostReadyPromise = new Promise<void>(resolve => { resolveHandoffHostReady = resolve })
  const unlistenHostReady = await current.listen('handoff-host-ready', () => {
    handoffHostReady = true
    resolveHandoffHostReady?.()
  })
  const unlistenResolved = await current.listen<{ id?: string }>('handoff-resolved', event => {
    const id = String(event.payload?.id || '')
    if (handoffQueue.release(id)) void openNextHandoff()
  })

  const ensureHandoffWindow = async (): Promise<InstanceType<typeof WebviewWindow>> => {
    const existing = await WebviewWindow.getByLabel('handoff-host')
    if (existing) return existing
    const child = new WebviewWindow('handoff-host', {
      url: 'index.html?handoffHost=1',
      title: '下载文件信息 - HLS Downloader',
      width: 420,
      height: 460,
      minWidth: 380,
      minHeight: 360,
      center: true,
      resizable: true,
      decorations: false,
      alwaysOnTop: false,
      focus: false,
      visible: false,
    })
    await new Promise<void>((resolve, reject) => {
      void child.once('tauri://created', () => resolve())
      void child.once('tauri://error', event => reject(new Error(String(event.payload || '无法创建下载确认窗口'))))
    })
    return child
  }

  // Warm one hidden WebView during desktop startup. A later click only swaps
  // its handoff ID and shows/focuses it, eliminating per-click WebView startup.
  await ensureHandoffWindow()
  if (!handoffHostReady) {
    await Promise.race([
      handoffHostReadyPromise,
      new Promise(resolve => window.setTimeout(resolve, 3000)),
    ])
  }

  const showMain = async () => {
    await current.show().catch(() => {})
    await current.unminimize().catch(() => {})
    await current.setFocus().catch(() => {})
  }

  const openNextHandoff = async (): Promise<void> => {
    if (stopped || openingHandoff || handoffQueue.activeId) return
    const id = handoffQueue.begin()
    if (!id) return
    openingHandoff = true
    try {
      const child = await ensureHandoffWindow()
      await emitTo('handoff-host', 'handoff-request', { id })
      await child.show().catch(() => {})
      await child.unminimize().catch(() => {})
      await child.setFocus().catch(() => {})
      await localRequest(`/desktop/handoffs/${encodeURIComponent(id)}/presented`, { method: 'POST', body: '{}' })
    } catch {
      handoffQueue.release(id)
      await localRequest(`/browser/handoffs/${encodeURIComponent(id)}/cancel`, { method: 'POST', body: '{}' }).catch(() => {})
    } finally {
      openingHandoff = false
      void openNextHandoff()
    }
  }

  const openHandoff = async (id: string) => {
    if (!id) return
    if (id === handoffQueue.activeId) {
      const child = await ensureHandoffWindow().catch(() => null)
      await child?.show().catch(() => {})
      await child?.setFocus().catch(() => {})
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
    unlistenHostReady()
    unlistenResolved()
    void WebviewWindow.getByLabel('handoff-host').then(window => window?.destroy()).catch(() => {})
    void localRequest('/desktop/session/stop', { method: 'POST', body: '{}' }).catch(() => {})
  }
}
