export interface TaskLike {
  id: string
  status: string
  output_path?: string
  output_is_file?: boolean
  available_actions?: string[]
  is_live?: boolean
  task_type?: 'hls' | 'dash' | 'http' | 'torrent'
  playback_ready?: boolean
}

export interface CommandState {
  start: boolean
  pause: boolean
  resume: boolean
  cancel: boolean
  retry: boolean
  delete: boolean
  open: boolean
  log: boolean
  pauseLabel: string
  resumeLabel: string
}

/** Stopping a live recording is irreversible, so the pause/resume wording
 *  must reflect every targeted task, not just the clicked row. */
export function pauseLabelFor(tasks: Pick<TaskLike, 'is_live'>[]): string {
  const liveCount = tasks.filter(task => task.is_live).length
  if (!liveCount) return '暂停'
  if (liveCount === tasks.length) return '停止录制'
  return '暂停 / 停止录制'
}

export function resumeLabelFor(tasks: Pick<TaskLike, 'is_live'>[]): string {
  const liveCount = tasks.filter(task => task.is_live).length
  if (!liveCount) return '恢复'
  if (liveCount === tasks.length) return '继续录制'
  return '恢复 / 继续录制'
}

const PAUSABLE = new Set(['downloading_segments', 'downloading', 'fetching_metadata', 'checking'])
const CANCELABLE = new Set([
  'queued', 'downloading', 'downloading_m3u8', 'parsing',
  'downloading_segments', 'fetching_metadata', 'checking', 'awaiting_selection',
  'awaiting_confirmation', 'pausing', 'paused', 'merging', 'remuxing',
])
const RETRYABLE = new Set(['failed', 'canceled', 'unsupported'])

export function commandState(tasks: TaskLike[]): CommandState {
  if (!tasks.length) {
    return {
      start: false, pause: false, resume: false, cancel: false,
      retry: false, delete: false, open: false, log: false,
      pauseLabel: '暂停', resumeLabel: '恢复',
    }
  }
  const backendActions = tasks.every(task => Array.isArray(task.available_actions))
  const allowed = (action: string) => backendActions
    ? tasks.every(task => task.available_actions!.includes(action))
    : null
  return {
    start: allowed('start') ?? tasks.every(task => task.status === 'queued'),
    pause: allowed('pause') ?? tasks.every(task => PAUSABLE.has(task.status)),
    resume: allowed('resume') ?? tasks.every(task => task.status === 'paused'),
    cancel: allowed('cancel') ?? tasks.every(task => CANCELABLE.has(task.status)),
    retry: allowed('retry') ?? tasks.every(task => RETRYABLE.has(task.status)),
    delete: allowed('delete') ?? true,
    open: tasks.length === 1 && (allowed('open') ?? tasks[0].status === 'done'),
    log: tasks.length === 1 && (allowed('log') ?? true),
    pauseLabel: pauseLabelFor(tasks),
    resumeLabel: resumeLabelFor(tasks),
  }
}
