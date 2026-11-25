// 「主程序」按钮的启动阶段。每个阶段都对应一层真实回执：
//   connecting  浏览器 → Native Host 端口（宿主是否注册、能否连上 Core）
//   launching   已请求桌面打开窗口，等待相同启动编号的回执
//   ready       工作台窗口已显示
//   requested   旧版 Core 不支持就绪回执，只能说“已请求打开”
// 失败按可恢复操作分类，而不是只给一句“无法打开主程序”。

export type LaunchFailure =
  | 'host_missing'
  | 'host_forbidden'
  | 'host_disconnected'
  | 'core_unreachable'
  | 'workbench_spawn_failed'
  | 'workbench_failed'
  | 'launch_status_lost'
  | 'timeout'
  | 'unknown'

export type LaunchStage = 'connecting' | 'launching' | 'ready' | 'requested' | 'failed'
export type LaunchAction = 'retry' | 'keep_waiting' | 'install' | 'repair'

export interface LaunchState {
  stage: LaunchStage
  failure?: LaunchFailure
  detail?: string
  pending?: LaunchReceipt
}

export interface LaunchReceipt {
  launchId: string
  sinceSequence: number
}

export interface LaunchView {
  tone: 'progress' | 'success' | 'info' | 'error'
  title: string
  detail: string
  actions: LaunchAction[]
}

export const DESKTOP_RELEASES_URL = 'https://github.com/ciaooo55/hls-downloader/releases'
export const LAUNCH_READY_TIMEOUT_MS = 15_000

const FAILURE_CODES: ReadonlySet<string> = new Set<LaunchFailure>([
  'host_missing', 'host_forbidden', 'host_disconnected', 'core_unreachable',
  'workbench_spawn_failed', 'workbench_failed', 'launch_status_lost', 'timeout',
])

export function launchFailureCode(value: unknown): LaunchFailure {
  const code = String(value || '')
  return FAILURE_CODES.has(code) ? code as LaunchFailure : 'unknown'
}

export function launchView(state: LaunchState): LaunchView {
  switch (state.stage) {
    case 'connecting':
      return { tone: 'progress', title: '正在连接主程序…', detail: '检查浏览器集成和下载引擎。', actions: [] }
    case 'launching':
      return { tone: 'progress', title: '正在打开工作台…', detail: '窗口出现并可操作后，这里会显示「已打开」。', actions: [] }
    case 'ready':
      return { tone: 'success', title: '工作台已打开', detail: '', actions: [] }
    case 'requested':
      return {
        tone: 'info',
        title: '已请求打开工作台',
        detail: '当前桌面端版本不会回报窗口是否就绪；若未看到窗口，请更新桌面端。',
        actions: ['retry'],
      }
    case 'failed':
      return failureView(state.failure || 'unknown', state.detail || '')
  }
}

function failureView(failure: LaunchFailure, detail: string): LaunchView {
  switch (failure) {
    case 'host_missing':
      return {
        tone: 'error',
        title: '未检测到 HLS Downloader 桌面端',
        detail: '浏览器找不到主程序的连接组件。请安装或修复桌面端，然后重试。',
        actions: ['install', 'retry'],
      }
    case 'host_forbidden':
      return {
        tone: 'error',
        title: '浏览器不允许连接主程序',
        detail: '当前插件没有在桌面端注册。请重新运行桌面端安装程序修复浏览器集成。',
        actions: ['repair', 'retry'],
      }
    case 'host_disconnected':
      return {
        tone: 'error',
        title: '与主程序的连接中断',
        detail: '连接组件意外退出。重试会重新建立连接。',
        actions: ['retry'],
      }
    case 'core_unreachable':
      return {
        tone: 'error',
        title: '下载引擎没有响应',
        detail: '主程序的下载引擎未启动或已退出。重试会再次启动它。',
        actions: ['retry'],
      }
    case 'workbench_spawn_failed':
      return {
        tone: 'error',
        title: '工作台程序无法启动',
        detail: (detail ? `${detail}。` : '') + '可能是安装文件缺失或被安全软件拦截，请修复安装后重试。',
        actions: ['retry', 'repair'],
      }
    case 'workbench_failed':
      return {
        tone: 'error',
        title: '工作台启动时出错',
        detail: '详细原因已写入主程序数据目录的 logs/presenter-startup.json。',
        actions: ['retry', 'repair'],
      }
    case 'launch_status_lost':
      return { tone: 'error', title: '无法确认工作台是否已打开', detail: '桌面端尚未就绪，请重新打开。', actions: ['retry'] }
    case 'timeout':
      return {
        tone: 'error',
        title: '工作台 15 秒内未就绪',
        detail: '电脑繁忙或窗口被其他程序遮挡时会出现。可继续等待，或重新打开。',
        actions: ['keep_waiting', 'retry'],
      }
    default:
      return {
        tone: 'error',
        title: '无法打开主程序',
        detail: detail || '请重试；仍失败时从开始菜单启动 HLS Downloader。',
        actions: ['retry'],
      }
  }
}

export interface LaunchDeps {
  send(message: Record<string, unknown>): Promise<any>
  onState(state: LaunchState): void
  now?: () => number
  sleep?: (ms: number) => Promise<void>
  timeoutMs?: number
  /** Resume waiting on an earlier launch instead of requesting a new one. */
  resumeFrom?: LaunchReceipt
}

function failed(failure: LaunchFailure, detail = '', pending?: LaunchReceipt): LaunchState {
  return { stage: 'failed', failure, detail, pending }
}

/**
 * Drives one click on 「主程序」. Status polls do not wait in the host,
 * so download handoffs can share the same Native Messaging connection.
 */
export async function launchDesktop(deps: LaunchDeps): Promise<LaunchState> {
  const now = deps.now ?? Date.now
  const sleep = deps.sleep ?? (ms => new Promise<void>(resolve => setTimeout(resolve, ms)))
  const deadline = now() + (deps.timeoutMs ?? LAUNCH_READY_TIMEOUT_MS)
  const emit = (state: LaunchState) => { deps.onState(state); return state }

  let pending = deps.resumeFrom
  if (!pending) {
    emit({ stage: 'connecting' })
    let response: any
    try {
      response = await deps.send({ type: 'activate' })
    } catch (error) {
      return emit(failed(launchFailureCode((error as { code?: unknown })?.code), errorText(error)))
    }
    if (!response?.ok) return emit(failed(launchFailureCode(response?.code), String(response?.error || '')))
    if (response.stage === 'requested' || response.verifiable === false) return emit({ stage: 'requested' })
    const sinceSequence = response.since_sequence
    const launchId = response.launch_id
    if (!Number.isSafeInteger(sinceSequence) || typeof launchId !== 'string' || !launchId) {
      return emit({ stage: 'requested' })
    }
    pending = { sinceSequence, launchId }
  }

  emit({ stage: 'launching', pending })
  while (now() < deadline) {
    let status: any
    try {
      status = await deps.send({ type: 'workbench-status', ...pending })
    } catch (error) {
      return emit(failed(launchFailureCode((error as { code?: unknown })?.code), errorText(error), pending))
    }
    if (!status?.ok) return emit(failed(launchFailureCode(status?.code), String(status?.error || ''), pending))
    if (status.ready === true) return emit({ stage: 'ready' })
    if (status.failed === true) return emit(failed('workbench_failed'))
    const next = Number(status.since_sequence)
    if (Number.isSafeInteger(next) && next > pending.sinceSequence) {
      pending = { ...pending, sinceSequence: next }
    }
    await sleep(300)
  }
  return emit(failed('timeout', '', pending))
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error || '')
}
