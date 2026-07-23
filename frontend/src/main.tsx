import React from 'react'
import ReactDOM from 'react-dom/client'
import './styles/app.css'
import './styles.css'
import './cockpit-shell.css'
import { prepareTauriRuntime } from './tauri'

const params = new URLSearchParams(window.location.search)
const handoffId = params.get('handoff')?.trim() || ''
const backgroundHost = params.get('host') === '1'
const root = ReactDOM.createRoot(document.getElementById('root')!)

function BootScreen({ label }: { label: string }) {
  return (
    <main className="desktop-boot-screen">
      <i className="desktop-boot-spinner" />
      <span>{label}</span>
    </main>
  )
}

function renderFailure(reason: unknown) {
  const message = reason instanceof Error ? reason.message : String(reason || '界面加载失败')
  root.render(
    <main className="desktop-boot-screen desktop-boot-error">
      <strong>界面加载失败</strong>
      <span>{message}</span>
    </main>,
  )
}

async function boot() {
  await prepareTauriRuntime()
  if (backgroundHost) {
    document.documentElement.dataset.surface = 'host'
    root.render(<BootScreen label="HLS Downloader 后台服务已就绪" />)
  } else if (handoffId) {
    document.documentElement.dataset.surface = 'handoff'
    root.render(<BootScreen label="正在准备下载窗口" />)
    const { default: BrowserHandoffWindow } = await import('./BrowserHandoffWindow')
    root.render(
      <React.StrictMode>
        <BrowserHandoffWindow handoffId={handoffId} />
      </React.StrictMode>,
    )
  } else {
    root.render(<BootScreen label="正在打开下载管理器" />)
    const { default: App } = await import('./App')
    root.render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    )
  }
}

void boot().catch(renderFailure)
