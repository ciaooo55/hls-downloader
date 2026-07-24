import { Minus, Square, X } from 'lucide-react'

async function controlWindow(action: 'minimize' | 'maximize' | 'close') {
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  const current = getCurrentWindow()
  if (action === 'minimize') await current.minimize()
  else if (action === 'maximize') await current.toggleMaximize()
  else await current.close()
}

export default function WindowChrome() {
  return (
    <header className="hls-window-chrome">
      <div className="hls-window-drag-region" data-tauri-drag-region>
        <img className="hls-window-chrome-mark" src="./app-icon.png" alt="" />
        <span>HLS Downloader</span>
      </div>
      <div className="hls-window-controls" aria-label="窗口控制">
        <button type="button" aria-label="最小化" title="最小化" onClick={() => void controlWindow('minimize')}><Minus size={16} /></button>
        <button type="button" aria-label="最大化或还原" title="最大化或还原" onClick={() => void controlWindow('maximize')}><Square size={13} /></button>
        <button type="button" className="hls-window-close" aria-label="关闭窗口" title="关闭窗口" onClick={() => void controlWindow('close')}><X size={17} /></button>
      </div>
    </header>
  )
}
