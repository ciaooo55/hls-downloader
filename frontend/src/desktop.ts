import { coreOrigin, isTauriDesktop } from './tauri'

interface NativeResult {
  ok: boolean
  canceled?: boolean
  path?: string
  error?: string
  installed?: boolean
  mode?: string
  shell?: string
  desktop_version?: string
  browser_opened?: boolean
}

interface NativeApi {
  open_browser_extension_installer(): Promise<NativeResult>
  get_desktop_info(): Promise<NativeResult>
  begin_uninstall(): Promise<NativeResult>
  close_window(): Promise<NativeResult>
  choose_folder?(directory?: string): Promise<NativeResult>
}

declare global {
  interface Window {
    pywebview?: { api: NativeApi }
  }
}

async function waitForNativeApi(timeoutMs = 2500): Promise<NativeApi | null> {
  if (window.pywebview?.api) return window.pywebview.api
  return new Promise(resolve => {
    const timer = window.setTimeout(() => resolve(null), timeoutMs)
    const onReady = () => {
      window.clearTimeout(timer)
      resolve(window.pywebview?.api || null)
    }
    window.addEventListener('pywebviewready', onReady, { once: true })
  })
}

export async function openBrowserExtensionInstaller(): Promise<NativeResult> {
  try {
    if (isTauriDesktop()) {
      const [{ invoke }, { join }, { openPath }] = await Promise.all([
        import('@tauri-apps/api/core'),
        import('@tauri-apps/api/path'),
        import('@tauri-apps/plugin-opener'),
      ])
      const root = await invoke<string>('get_app_root')
      const path = await join(root, 'browser-extension', 'chrome')
      await openPath(path)
      return { ok: true, path }
    }
    const api = await waitForNativeApi()
    if (!api) return { ok: false, error: '扩展安装工具仅在桌面版中可用' }
    return await api.open_browser_extension_installer()
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法打开扩展安装工具' }
  }
}

export async function getDesktopInfo(): Promise<NativeResult> {
  try {
    if (isTauriDesktop()) {
      const { invoke } = await import('@tauri-apps/api/core')
      return await invoke<NativeResult>('get_desktop_info')
    }
    const api = await waitForNativeApi()
    if (!api) return { ok: true, installed: false, mode: 'web' }
    return await api.get_desktop_info()
  } catch (reason) {
    return { ok: false, installed: false, error: reason instanceof Error ? reason.message : '无法读取桌面版信息' }
  }
}

export async function beginUninstall(): Promise<NativeResult> {
  try {
    if (isTauriDesktop()) {
      const { invoke } = await import('@tauri-apps/api/core')
      return await invoke<NativeResult>('begin_uninstall')
    }
    const api = await waitForNativeApi()
    if (!api) return { ok: false, error: '卸载仅在安装版中可用' }
    return await api.begin_uninstall()
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法启动卸载程序' }
  }
}




export async function pickFolder(directory = ''): Promise<NativeResult> {
  try {
    if (isTauriDesktop()) {
      const { open } = await import('@tauri-apps/plugin-dialog')
      const path = await open({ directory: true, multiple: false, defaultPath: directory || undefined })
      return path ? { ok: true, path } : { ok: false, canceled: true }
    }
    const api = await waitForNativeApi(1000)
    if (!api?.choose_folder) return { ok: false, error: 'native-folder-unavailable' }
    return await api.choose_folder(directory)
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法打开文件夹选择对话框' }
  }
}

export async function pickLocalMediaFile(): Promise<NativeResult> {
  try {
    if (isTauriDesktop()) {
      const { open } = await import('@tauri-apps/plugin-dialog')
      const path = await open({ multiple: false, title: '选择要推送到电视的本机文件' })
      return path ? { ok: true, path } : { ok: false, canceled: true }
    }
    return { ok: false, error: '本机文件推送仅在桌面版中可用' }
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法打开文件选择对话框' }
  }
}

export async function closeDesktopWindow(): Promise<NativeResult> {
  try {
    if (isTauriDesktop()) {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      await getCurrentWindow().destroy()
      return { ok: true }
    }
    const api = await waitForNativeApi(1000)
    if (api?.close_window) return await api.close_window()
    window.close()
    return { ok: true }
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法关闭窗口' }
  }
}
