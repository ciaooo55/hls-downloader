import { useEffect, useState } from 'react'
import { Maximize2, Minimize2, Minus, X } from 'lucide-react'

type ResizeDirection = 'East' | 'North' | 'NorthEast' | 'NorthWest' | 'South' | 'SouthEast' | 'SouthWest' | 'West'

const RESIZE_GRIPS: Array<{ direction: ResizeDirection, className: string }> = [
  { direction: 'North', className: 'north' },
  { direction: 'South', className: 'south' },
  { direction: 'East', className: 'east' },
  { direction: 'West', className: 'west' },
  { direction: 'NorthEast', className: 'north-east' },
  { direction: 'NorthWest', className: 'north-west' },
  { direction: 'SouthEast', className: 'south-east' },
  { direction: 'SouthWest', className: 'south-west' },
]

export function nextWindowSizeAction(isMaximized: boolean): 'maximize' | 'unmaximize' {
  return isMaximized ? 'unmaximize' : 'maximize'
}

async function controlWindow(action: 'minimize' | 'close') {
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  const current = getCurrentWindow()
  if (action === 'minimize') await current.minimize()
  else await current.close()
}

async function toggleWindowSize(): Promise<boolean> {
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  const current = getCurrentWindow()
  const action = nextWindowSizeAction(await current.isMaximized())
  if (action === 'maximize') await current.maximize()
  else await current.unmaximize()
  return current.isMaximized()
}

async function startDragging() {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await getCurrentWindow().startDragging()
  } catch {
    // The shared chrome also renders in the browser development surface.
  }
}

async function startResizing(direction: ResizeDirection) {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await getCurrentWindow().startResizeDragging(direction)
  } catch {
    // Resizing is only available in undecorated Tauri windows.
  }
}

export default function WindowChrome({ resizable = false }: { resizable?: boolean }) {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      const current = getCurrentWindow()
      const refresh = () => {
        void current.isMaximized().then(value => {
          if (!disposed) setMaximized(value)
        }).catch(() => undefined)
      }
      refresh()
      unlisten = await current.onResized(refresh)
    }).catch(() => undefined)
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const changeWindowSize = () => {
    void toggleWindowSize().then(setMaximized).catch(() => undefined)
  }

  return (
    <>
      <header className="hls-window-chrome">
        <div className="hls-window-drag-region" onMouseDown={event => {
          if (event.button === 0) void startDragging()
        }} onDoubleClick={changeWindowSize}>
          <img className="hls-window-chrome-mark" src="./app-icon.png" alt="" />
          <span>HLS Downloader</span>
        </div>
        <div className="hls-window-controls" aria-label="窗口控制">
          <button type="button" aria-label="最小化" title="最小化" onClick={() => void controlWindow('minimize').catch(() => undefined)}><Minus size={16} /></button>
          <button type="button" aria-label={maximized ? '还原' : '最大化'} title={maximized ? '还原' : '最大化'} onClick={changeWindowSize}>
            {maximized ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
          </button>
          <button type="button" className="hls-window-close" aria-label="关闭窗口" title="关闭窗口" onClick={() => void controlWindow('close').catch(() => undefined)}><X size={17} /></button>
        </div>
      </header>
      {resizable && <div className="hls-window-resize-grips" aria-hidden="true">
        {RESIZE_GRIPS.map(({ direction, className }) => <div
          key={direction}
          className={`hls-window-resize-grip ${className}`}
          onPointerDown={event => {
            if (event.button !== 0) return
            event.preventDefault()
            event.stopPropagation()
            void startResizing(direction)
          }}
        />)}
      </div>}
    </>
  )
}
