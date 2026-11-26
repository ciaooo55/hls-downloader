export const EXTENSION_PRODUCT_LABEL = '浏览器插件'

export type EngineConnectionState =
  | 'online'
  | 'user_shutdown'
  | 'host_missing'
  | 'core_unreachable'

export interface EngineStatusCopy {
  state: EngineConnectionState
  tone: 'online' | 'standby' | 'missing' | 'offline'
  label: string
  helperTitle: string
  helperDetail: string
}

export function extensionVersionLabel(version: string): string {
  return `版本 ${version}`
}

export function engineConnectionLabel(online: boolean, reconnecting: boolean): string {
  if (online) return '下载引擎已连接'
  return reconnecting ? '下载引擎正在重连' : '下载引擎未连接'
}

export function classifyEngineConnectionState(response: unknown): EngineConnectionState {
  if (response && typeof response === 'object' && (response as { ok?: unknown }).ok === true) {
    return 'online'
  }
  const record = response && typeof response === 'object' ? response as Record<string, unknown> : {}
  const code = String(record.error_code || record.code || '').trim().toLowerCase()
  const message = String(record.error || record.message || '')
  if (code === 'user_shutdown' || /user[_-]shutdown|已退出/i.test(message)) {
    return 'user_shutdown'
  }
  if (
    code === 'host_missing'
    || code === 'host_forbidden'
    || /native messaging host not found|no such native application|未检测到.*桌面端|禁止访问下载器/i.test(message)
  ) {
    return 'host_missing'
  }
  return 'core_unreachable'
}

export function engineStatusCopy(state: EngineConnectionState): EngineStatusCopy {
  switch (state) {
    case 'online':
      return {
        state,
        tone: 'online',
        label: '引擎已就绪',
        helperTitle: '引擎已就绪',
        helperDetail: '',
      }
    case 'user_shutdown':
      return {
        state,
        tone: 'standby',
        label: '桌面端已退出 · 插件下载可自动唤醒',
        helperTitle: '桌面端已退出 · 插件下载可自动唤醒',
        helperDetail: '从托盘退出后，普通浏览器下载将保留在浏览器中不打扰；点击插件下载按钮或右键菜单项会自动唤醒桌面端。',
      }
    case 'host_missing':
      return {
        state,
        tone: 'missing',
        label: '未检测到桌面端 · 请先安装或运行桌面程序',
        helperTitle: '未检测到桌面端 · 请先安装或运行桌面程序',
        helperDetail: '浏览器尚未发现本地连接组件，请先安装或启动一次桌面端完成注册。',
      }
    case 'core_unreachable':
      return {
        state,
        tone: 'offline',
        label: '连接中断 · 点击重试或打开桌面端',
        helperTitle: '连接中断 · 点击重试或打开桌面端',
        helperDetail: '后台服务暂未响应，可点击重试检测或直接拉起桌面主程序。',
      }
  }
}

/**
 * User-facing text for a caught value. `String(new Error('x'))` yields
 * "Error: x", which leaked the JavaScript class name into popup/overlay
 * messages (e.g. "Error: 下载器连接已断开").
 */
export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message || error.name
  return String(error ?? '')
}
