/**
 * 字节数与秒数的显示格式。
 *
 * 在此之前，三个入口各抄了一份：后台内容面板（content.ts）、媒体浮层
 * （lib/mediaOverlay.ts）、弹窗（entrypoints/popup/main.ts）。单位阶梯、进位阈值、
 * 小数位由三处分别维护——改一处忘记另两处，就会出现「同一个文件在浮层里显示
 * 2.2 GB、在面板里显示 2304 MB」这种对不上的情况。
 *
 * 边界约定：
 * - 未知值显示什么**不**由这里决定，调用方自己判断（面板是「大小未知」，浮层和
 *   弹窗在调用点就已经把 0 过滤掉了）。所以这里只保留「正数长什么样」。
 * - 只有 [formatDuration] 保留 `<= 0` 返回空串，因为浮层的调用点会直接把
 *   `Number(resource.duration || 0)` 传进来，不能假定它一定为正。
 */

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const

/** 正数字节数的单位阶梯：>= 100 取整数，否则保留一位小数。 */
export function formatBytes(bytes: number): string {
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value >= 100 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${BYTE_UNITS[unit]}`
}

/** 时长一律 mm:ss，超过一小时补 h:mm:ss。非正数与不可用值返回空串。 */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return ''
  const total = Math.round(seconds)
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const remainder = total % 60
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${String(remainder).padStart(2, '0')}`
    : `${minutes}:${String(remainder).padStart(2, '0')}`
}
