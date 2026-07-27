import { Minus, Square, X } from 'lucide-react'

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

async function controlWindow(action: 'minimize' | 'maximize' | 'close') {
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  const current = getCurrentWindow()
  if (action === 'minimize') await current.minimize()
  else if (action === 'maximize') await current.toggleMaximize()
  else await current.close()
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
  return (
    <>
      <header className="hls-window-chrome">
        <div className="hls-window-drag-region" onMouseDown={event => {
          if (event.button === 0) void startDragging()
        }}>
          <img className="hls-window-chrome-mark" src="./app-icon.png" alt="" />
          <span>HLS Downloader</span>
        </div>
        <div className="hls-window-controls" aria-label="窗口控制">
          <button type="button" aria-label="最小化" title="最小化" onClick={() => void controlWindow('minimize')}><Minus size={16} /></button>
          <button type="button" aria-label="最大化或还原" title="最大化或还原" onClick={() => void controlWindow('maximize')}><Square size={13} /></button>
          <button type="button" className="hls-window-close" aria-label="关闭窗口" title="关闭窗口" onClick={() => void controlWindow('close')}><X size={17} /></button>
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
