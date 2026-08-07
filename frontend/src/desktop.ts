import { isTauriDesktop } from './tauri'

export const FIREFOX_ADDON_URL = 'https://addons.mozilla.org/zh-CN/firefox/addon/hls_downloader/'

export interface NativeResult {
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

function unavailable(error: string): NativeResult {
  return { ok: false, error }
}

export async function openBrowserExtensionInstaller(): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) return unavailable('扩展安装工具仅在桌面版中可用')
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<NativeResult>('open_browser_extension_installer')
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法打开扩展安装工具')
  }
}

/** Opens the published AMO page. The native implementation uses the default
 * browser and intentionally accepts no caller-provided URL. */
export async function openFirefoxAddonPage(): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) {
      window.open(FIREFOX_ADDON_URL, '_blank', 'noopener,noreferrer')
      return { ok: true }
    }
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<NativeResult>('open_firefox_addon_page')
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法打开 Firefox Add-ons 安装页')
  }
}

export async function getDesktopInfo(): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) return { ok: true, installed: false, mode: 'web' }
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<NativeResult>('get_desktop_info')
  } catch (reason) {
    return {
      ok: false,
      installed: false,
      error: reason instanceof Error ? reason.message : '无法读取桌面版信息',
    }
  }
}

export async function beginUninstall(): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) return unavailable('卸载仅在安装版中可用')
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<NativeResult>('begin_uninstall')
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法启动卸载程序')
  }
}

export async function pickFolder(directory = ''): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) return unavailable('native-folder-unavailable')
    const { open } = await import('@tauri-apps/plugin-dialog')
    const path = await open({ directory: true, multiple: false, defaultPath: directory || undefined })
    return path ? { ok: true, path } : { ok: false, canceled: true }
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法打开文件夹选择对话框')
  }
}

export async function pickLocalMediaFile(): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) return unavailable('本机文件投屏仅在桌面版中可用')
    const { open } = await import('@tauri-apps/plugin-dialog')
    const path = await open({ multiple: false, title: '选择要投屏或 TVBox 推送的本机文件' })
    return path ? { ok: true, path } : { ok: false, canceled: true }
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法打开文件选择对话框')
  }
}

export async function closeDesktopWindow(): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) {
      window.close()
      return { ok: true }
    }
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await getCurrentWindow().destroy()
    return { ok: true }
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法关闭窗口')
  }
}

export async function quitApplication(): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) {
      window.close()
      return { ok: true }
    }
    const { exit } = await import('@tauri-apps/plugin-process')
    await exit(0)
    return { ok: true }
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法退出应用')
  }
}

export async function resizeDesktopWindow(width: number, height: number): Promise<NativeResult> {
  try {
    if (!isTauriDesktop()) return unavailable('native-resize-unavailable')
    const [{ getCurrentWindow }, { LogicalSize }] = await Promise.all([
      import('@tauri-apps/api/window'),
      import('@tauri-apps/api/dpi'),
    ])
    await getCurrentWindow().setSize(new LogicalSize(width, height))
    return { ok: true }
  } catch (reason) {
    return unavailable(reason instanceof Error ? reason.message : '无法调整窗口大小')
  }
}
