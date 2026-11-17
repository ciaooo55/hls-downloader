export const EXTENSION_PRODUCT_LABEL = '浏览器插件'

export function extensionVersionLabel(version: string): string {
  return `版本 ${version}`
}

export function engineConnectionLabel(online: boolean, reconnecting: boolean): string {
  if (online) return '下载引擎已连接'
  return reconnecting ? '下载引擎正在重连' : '下载引擎未连接'
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
