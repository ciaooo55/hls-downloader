import {
  COMPLETE_WINDOW_HEIGHT,
  COMPLETE_WINDOW_LABEL,
  COMPLETE_WINDOW_WIDTH,
  enqueueCompleteItem,
  PROGRESS_WINDOW_LABEL,
  PROGRESS_WINDOW_WIDTH,
  progressWindowHeight,
  pruneDismissedProgressIds,
  shouldShowProgressWindow,
  type DownloadCompleteItem,
  type DownloadProgressItem,
} from './downloadOverlay'

type WebviewWindowCtor = typeof import('@tauri-apps/api/webviewWindow').WebviewWindow

let WebviewWindow: WebviewWindowCtor | null = null
let overlayReady = false
let dismissedProgressIds = new Set<string>()
let latestProgress: DownloadProgressItem[] = []
let pendingComplete: DownloadCompleteItem[] = []
let progressEnabled = true
let completeEnabled = true

async function getWindow(label: string) {
  if (!WebviewWindow) return null
  return WebviewWindow.getByLabel(label)
}

async function placeBottomRight(label: string, width: number, height: number, extraBottom = 16) {
  const child = await getWindow(label)
  if (!child) return
  const [{ LogicalPosition, LogicalSize }, { currentMonitor }] = await Promise.all([
    import('@tauri-apps/api/dpi'),
    import('@tauri-apps/api/window'),
  ])
  await child.setSize(new LogicalSize(width, height)).catch(() => {})
  try {
    const monitor = await currentMonitor()
    if (!monitor) {
      await child.center().catch(() => {})
      return
    }
    const workSize = monitor.workArea.size.toLogical(monitor.scaleFactor)
    const workPos = monitor.workArea.position.toLogical(monitor.scaleFactor)
    const x = Math.max(workPos.x + 12, workPos.x + workSize.width - width - 16)
    const y = Math.max(workPos.y + 12, workPos.y + workSize.height - height - extraBottom)
    await child.setPosition(new LogicalPosition(x, y))
  } catch {
    await child.center().catch(() => {})
  }
}

async function hideWindow(label: string) {
  const child = await getWindow(label)
  await child?.hide().catch(() => {})
}

async function showProgressWindow(tasks: DownloadProgressItem[]) {
  const child = await getWindow(PROGRESS_WINDOW_LABEL)
  if (!child) return
  const { emitTo } = await import('@tauri-apps/api/event')
  await emitTo(PROGRESS_WINDOW_LABEL, 'download-progress-sync', { tasks })
  const height = progressWindowHeight(tasks.length)
  await child.setAlwaysOnTop(true).catch(() => {})
  await placeBottomRight(PROGRESS_WINDOW_LABEL, PROGRESS_WINDOW_WIDTH, height)
  // focusable:false keeps this from stealing the browser while a takeover
  // download is running; show() still paints the always-on-top box.
  await child.show().catch(() => {})
}

async function showCompleteWindow() {
  const child = await getWindow(COMPLETE_WINDOW_LABEL)
  if (!child) return
  const { LogicalSize } = await import('@tauri-apps/api/dpi')
  await child.setAlwaysOnTop(true).catch(() => {})
  await child.setSize(new LogicalSize(COMPLETE_WINDOW_WIDTH, COMPLETE_WINDOW_HEIGHT)).catch(() => {})
  await child.center().catch(() => {})
  await child.show().catch(() => {})
  await child.unminimize().catch(() => {})
  await child.setFocus().catch(() => {})
}

async function flushPendingOverlays(): Promise<void> {
  if (!overlayReady) return
  if (pendingComplete.length) {
    const { emitTo } = await import('@tauri-apps/api/event')
    for (const item of pendingComplete) {
      await emitTo(COMPLETE_WINDOW_LABEL, 'download-complete-enqueue', { item })
    }
    pendingComplete = []
  }
  await applyDownloadProgressWindow()
}

export async function applyDownloadProgressWindow(): Promise<void> {
  if (!overlayReady) return
  const runningIds = latestProgress.map(task => task.id)
  dismissedProgressIds = pruneDismissedProgressIds(dismissedProgressIds, runningIds)
  if (!progressEnabled || !shouldShowProgressWindow(runningIds, dismissedProgressIds)) {
    const child = await getWindow(PROGRESS_WINDOW_LABEL)
    const { emitTo } = await import('@tauri-apps/api/event')
    await emitTo(PROGRESS_WINDOW_LABEL, 'download-progress-sync', { tasks: latestProgress }).catch(() => {})
    await child?.hide().catch(() => {})
    return
  }
  await showProgressWindow(latestProgress)
}

export async function syncDownloadProgressWindow(
  tasks: DownloadProgressItem[],
  enabled = true,
): Promise<void> {
  progressEnabled = enabled
  latestProgress = enabled ? tasks : []
  if (!enabled) dismissedProgressIds = new Set()
  await applyDownloadProgressWindow()
}

export async function enqueueDownloadCompletePopup(
  item: DownloadCompleteItem | null,
  enabled = true,
): Promise<void> {
  completeEnabled = enabled
  if (!enabled || !item?.id) return
  if (!overlayReady) {
    pendingComplete = enqueueCompleteItem(pendingComplete, item)
    return
  }
  const { emitTo } = await import('@tauri-apps/api/event')
  await emitTo(COMPLETE_WINDOW_LABEL, 'download-complete-enqueue', { item })
}

export async function setDownloadCompletePopupEnabled(enabled: boolean): Promise<void> {
  completeEnabled = enabled
  if (!enabled) await hideWindow(COMPLETE_WINDOW_LABEL)
}

export function rememberProgressWindowDismissed(ids: string[]): void {
  for (const id of ids) {
    if (id) dismissedProgressIds.add(id)
  }
  void applyDownloadProgressWindow()
}

export async function initDownloadOverlayWindows(
  WindowType: WebviewWindowCtor,
): Promise<() => void> {
  WebviewWindow = WindowType
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  const current = getCurrentWindow()

  const unlistenDismissed = await current.listen<{ ids?: string[] }>('download-progress-dismissed', event => {
    rememberProgressWindowDismissed(Array.isArray(event.payload?.ids) ? event.payload.ids : [])
  })
  const unlistenEmpty = await current.listen('download-complete-empty', () => {
    void hideWindow(COMPLETE_WINDOW_LABEL)
  })
  const unlistenCompleteReady = await current.listen('download-complete-ready', () => {
    if (completeEnabled) void showCompleteWindow()
  })
  let hostsReady = 0
  const markHostReady = () => {
    hostsReady += 1
    if (hostsReady < 2 || overlayReady) return
    overlayReady = true
    void flushPendingOverlays()
  }
  const unlistenProgressReady = await current.listen('download-progress-ready', markHostReady)
  const unlistenCompleteHostReady = await current.listen('download-complete-host-ready', markHostReady)

  const ensure = async (
    label: string,
    url: string,
    title: string,
    width: number,
    height: number,
    focusable: boolean,
  ) => {
    const existing = await WindowType.getByLabel(label)
    if (existing) return existing
    const child = new WindowType(label, {
      url,
      title,
      width,
      height,
      minWidth: width,
      minHeight: 120,
      center: label === COMPLETE_WINDOW_LABEL,
      resizable: false,
      decorations: false,
      alwaysOnTop: false,
      skipTaskbar: false,
      focus: false,
      focusable,
      visible: false,
      shadow: true,
    })
    await new Promise<void>((resolve, reject) => {
      void child.once('tauri://created', () => resolve())
      void child.once('tauri://error', event => reject(new Error(String(event.payload || `无法创建${title}`))))
    })
    return child
  }

  await Promise.all([
    ensure(
      PROGRESS_WINDOW_LABEL,
      'index.html?progressHost=1',
      '正在下载 - HLS Downloader',
      PROGRESS_WINDOW_WIDTH,
      progressWindowHeight(1),
      false,
    ),
    ensure(
      COMPLETE_WINDOW_LABEL,
      'index.html?completeHost=1',
      '下载完成 - HLS Downloader',
      COMPLETE_WINDOW_WIDTH,
      COMPLETE_WINDOW_HEIGHT,
      true,
    ),
  ])

  return () => {
    overlayReady = false
    unlistenDismissed()
    unlistenEmpty()
    unlistenCompleteReady()
    unlistenProgressReady()
    unlistenCompleteHostReady()
    void WindowType.getByLabel(PROGRESS_WINDOW_LABEL).then(window => window?.destroy()).catch(() => {})
    void WindowType.getByLabel(COMPLETE_WINDOW_LABEL).then(window => window?.destroy()).catch(() => {})
    WebviewWindow = null
  }
}
