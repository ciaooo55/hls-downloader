export interface TaskLike {
  id: string
  status: string
  output_path?: string
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
}

const PAUSABLE = new Set([
  'downloading', 'downloading_m3u8', 'parsing', 'downloading_segments',
])
const CANCELABLE = new Set([
  'queued', 'downloading', 'downloading_m3u8', 'parsing',
  'downloading_segments', 'pausing', 'paused', 'merging', 'remuxing',
])
const RETRYABLE = new Set(['failed', 'canceled', 'unsupported'])
const DELETABLE = new Set(['done', 'failed', 'canceled', 'unsupported'])

export function commandState(tasks: TaskLike[]): CommandState {
  if (!tasks.length) {
    return {
      start: false, pause: false, resume: false, cancel: false,
      retry: false, delete: false, open: false, log: false,
    }
  }
  return {
    start: tasks.every(task => task.status === 'queued'),
    pause: tasks.every(task => PAUSABLE.has(task.status)),
    resume: tasks.every(task => task.status === 'paused'),
    cancel: tasks.every(task => CANCELABLE.has(task.status)),
    retry: tasks.every(task => RETRYABLE.has(task.status)),
    delete: tasks.every(task => DELETABLE.has(task.status)),
    open: tasks.length === 1 && tasks[0].status === 'done',
    log: tasks.length === 1,
  }
}
