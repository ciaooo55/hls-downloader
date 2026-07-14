interface NativeResult {
  ok: boolean
  canceled?: boolean
  path?: string
  error?: string
  installed?: boolean
  mode?: string
}

interface NativeApi {
  export_userscript(): Promise<NativeResult>
  open_userscript_installer(): Promise<NativeResult>
  get_desktop_info(): Promise<NativeResult>
  begin_uninstall(): Promise<NativeResult>
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
