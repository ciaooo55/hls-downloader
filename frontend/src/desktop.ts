interface NativeResult {
  ok: boolean
  canceled?: boolean
  path?: string
  error?: string
  installed?: boolean
  mode?: string
  browser_opened?: boolean
}

interface NativeApi {
  export_userscript(): Promise<NativeResult>
  open_userscript_installer(): Promise<NativeResult>
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

export async function exportUserscript(): Promise<NativeResult> {
  try {
    const api = await waitForNativeApi()
    if (!api) return { ok: false, error: '脚本导出仅在桌面版中可用' }
    return await api.export_userscript()
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '脚本导出失败' }
  }
}

export async function openUserscriptInstaller(): Promise<NativeResult> {
  try {
    const api = await waitForNativeApi()
    if (api) return await api.open_userscript_installer()
    window.open('/userscript/m3u8-sniffer.user.js', '_blank', 'noopener')
    return { ok: true }
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法打开安装地址' }
  }
}

export async function openBrowserExtensionInstaller(): Promise<NativeResult> {
  try {
    const api = await waitForNativeApi()
    if (!api) return { ok: false, error: '扩展安装工具仅在桌面版中可用' }
    return await api.open_browser_extension_installer()
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法打开扩展安装工具' }
  }
}

export async function getDesktopInfo(): Promise<NativeResult> {
  try {
    const api = await waitForNativeApi()
    if (!api) return { ok: true, installed: false, mode: 'web' }
    return await api.get_desktop_info()
  } catch (reason) {
    return { ok: false, installed: false, error: reason instanceof Error ? reason.message : '无法读取桌面版信息' }
  }
}

export async function beginUninstall(): Promise<NativeResult> {
  try {
    const api = await waitForNativeApi()
    if (!api) return { ok: false, error: '卸载仅在安装版中可用' }
    return await api.begin_uninstall()
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法启动卸载程序' }
  }
}




export async function pickFolder(directory = ''): Promise<NativeResult> {
  try {
    const api = await waitForNativeApi(1000)
    if (!api?.choose_folder) return { ok: false, error: 'native-folder-unavailable' }
    return await api.choose_folder(directory)
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法打开文件夹选择对话框' }
  }
}

export async function closeDesktopWindow(): Promise<NativeResult> {
  try {
    const api = await waitForNativeApi(1000)
    if (api?.close_window) return await api.close_window()
    window.close()
    return { ok: true }
  } catch (reason) {
    return { ok: false, error: reason instanceof Error ? reason.message : '无法关闭窗口' }
  }
}
