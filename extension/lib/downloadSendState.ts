// 弹窗「下载」按钮的状态只来自 Core 的交接与任务事实，按同一个交接 ID
// （业务 ID）回查。弹窗关闭、插件重载或 Core 重启后重新打开，都从后台持久
// 记录恢复，而不是从零开始，从而不会因重复点击创建第二个任务。

export type SendState =
  | 'sending'      // 已发给后台，尚未拿到 Core 的交接回执
  | 'confirming'   // Core 已持久化交接，等待桌面确认窗
  | 'queued'       // 桌面已接受，任务已入队或正在下载
  | 'done'         // 任务完成且已落盘
  | 'canceled'     // 桌面端取消或拒绝；点击可重新发送
  | 'recoverable'  // 任务失败或暂停，续传状态在主程序里
  | 'checking'     // 暂时无法读取状态；不能当作失败，也不能再发一次
  | 'retry'        // 请求未被 Core 接收，可安全重试

export interface HandoffFacts {
  status?: string
  presentation?: string
  taskStatus?: string
  taskErrorCode?: string
}

export interface SendClickOptions {
  urlChanged?: boolean
  cooldownElapsed?: boolean
}

export const SEND_LABELS: Record<SendState, string> = {
  sending: '发送中',
  confirming: '等待确认',
  queued: '已加入',
  done: '已完成',
  canceled: '已取消',
  recoverable: '可恢复',
  checking: '核对中',
  retry: '重试',
}

export const SEND_TITLES: Record<SendState, string> = {
  sending: '正在发送到主程序，请稍候',
  confirming: '已交由桌面端等待确认，请在弹窗中选择开始或取消',
  queued: '已加入主程序下载队列，可在主程序查看进度',
  done: '主程序已完成下载',
  canceled: '已在桌面端取消；点击重新发送',
  recoverable: '下载中断，进度已保存；点击打开主程序继续，或点击右侧刷新地址重发',
  checking: '暂时无法确认交接状态，点击按原请求重新核对（不会重复下载）',
  retry: '上次未能加入队列，点击重试',
}

/** States in which a primary click must not create a duplicate handoff. */
export const SEND_LOCKED: ReadonlySet<SendState> = new Set(['sending', 'confirming', 'queued', 'done'])
/** States drawn with the spinner. */
export const SEND_PENDING: ReadonlySet<SendState> = new Set(['sending', 'confirming'])

const FAILED_TASK = new Set(['failed', 'error', 'paused', 'stopped'])
const DONE_TASK = new Set(['done', 'completed'])
const REFRESH_ARMED_CODES = new Set(['refresh_armed', 'arm_task_refresh', 'waiting_refresh'])

export function sendStateFromHandoff(facts: HandoffFacts): SendState {
  const status = String(facts.status || '')
  switch (status) {
    case 'pending':
      return facts.presentation === 'failed' || facts.presentation === 'fallback' ? 'retry' : 'confirming'
    case 'accepted': {
      const task = String(facts.taskStatus || '')
      const errorCode = String(facts.taskErrorCode || '').toLowerCase()
      if (REFRESH_ARMED_CODES.has(task.toLowerCase()) || REFRESH_ARMED_CODES.has(errorCode)) return 'retry'
      if (DONE_TASK.has(task)) return 'done'
      if (FAILED_TASK.has(task)) return 'recoverable'
      if (task === 'canceled' || task === 'cancelled') return 'canceled'
      return 'queued'
    }
    case 'rejected':
    case 'canceled':
      return 'canceled'
    case 'expired':
    case 'failed':
      return 'retry'
    default:
      return 'checking'
  }
}

/**
 * True when an item has already settled (`queued`, `done`, `recoverable`) and
 * may expose an explicit secondary "刷新地址 / 重新发送" affordance (e.g. for
 * Core `ArmTaskRefresh` or re-downloading) without risking an in-flight race.
 */
export function canForceResend(state: SendState | undefined): boolean {
  return state === 'queued' || state === 'done' || state === 'recoverable'
}

/** What a click does in each state. */
export function sendClickAction(
  state: SendState | undefined,
  options?: SendClickOptions,
): 'send' | 'recheck' | 'open_desktop' | 'none' {
  if (!state || state === 'retry' || state === 'canceled') return 'send'
  if (state === 'checking') return 'recheck'
  if (options?.urlChanged && (state === 'recoverable' || state === 'queued' || state === 'done')) {
    return 'send'
  }
  if (state === 'recoverable') return options?.cooldownElapsed ? 'send' : 'open_desktop'
  return 'none'
}
