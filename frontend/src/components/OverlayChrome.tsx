import { Minus, X } from 'lucide-react'

async function startDragging() {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await getCurrentWindow().startDragging()
  } catch {
    // The overlay chrome also renders in the browser development surface.
  }
}

async function minimizeWindow() {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await getCurrentWindow().minimize()
  } catch {
    // Minimize is only available in the desktop shell.
  }
}

export default function OverlayChrome({
  title,
  onClose,
  showMinimize = false,
}: {
  title: string
  onClose: () => void
  showMinimize?: boolean
}) {
  return (
    <header className="overlay-window-chrome">
      <div
        className="overlay-window-drag"
        onMouseDown={event => {
          if (event.button === 0) void startDragging()
        }}
      >
        <img className="hls-window-chrome-mark" src="./app-icon.png" alt="" />
        <span>{title}</span>
      </div>
      <div className="overlay-window-controls" aria-label="窗口控制">
        {showMinimize && (
          <button type="button" aria-label="最小化" title="最小化" onClick={() => void minimizeWindow()}>
            <Minus size={14} />
          </button>
        )}
        <button type="button" className="hls-window-close" aria-label="关闭" title="关闭" onClick={onClose}>
          <X size={15} />
        </button>
      </div>
    </header>
  )
}
