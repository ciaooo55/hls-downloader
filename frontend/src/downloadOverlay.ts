import { getDisplayedProgress, isRunningStatus } from './taskState'

export const PROGRESS_WINDOW_LABEL = 'download-progress'
export const COMPLETE_WINDOW_LABEL = 'download-complete'
export const PROGRESS_WINDOW_WIDTH = 392
export const COMPLETE_WINDOW_WIDTH = 400
export const COMPLETE_WINDOW_HEIGHT = 236
export const COMPLETE_QUEUE_CAP = 8
export const PROGRESS_MAX_VISIBLE = 4
export const PROGRESS_CHROME_HEIGHT = 30
export const PROGRESS_PAD = 10
export const PROGRESS_ROW_HEIGHT = 88

export interface DownloadProgressItem {
  id: string
  title: string
  filename: string
  status: string
  progress_percent: number
  downloaded_bytes: number
  total_bytes: number
  speed_bytes_per_sec: number
  eta_seconds: number
  available_actions?: string[]
  is_live?: boolean
}

export interface DownloadCompleteItem {
  id: string
  title: string
  filename: string
  output_path: string
  downloaded_bytes: number
  output_is_file: boolean
}

export function isProgressWindowTask(status?: string): boolean {
  return isRunningStatus(status)
}

export function selectProgressTasks<T extends { status?: string }>(tasks: T[]): T[] {
  return tasks.filter(task => isProgressWindowTask(task.status))
}

export function progressWindowHeight(count: number): number {
  const visible = Math.max(1, Math.min(PROGRESS_MAX_VISIBLE, Math.max(0, count)))
  return PROGRESS_CHROME_HEIGHT + PROGRESS_PAD + visible * PROGRESS_ROW_HEIGHT
}

export function shouldShowProgressWindow(runningIds: string[], dismissedIds: ReadonlySet<string>): boolean {
  if (!runningIds.length) return false
  return runningIds.some(id => !dismissedIds.has(id))
}

export function pruneDismissedProgressIds(
  dismissedIds: ReadonlySet<string>,
  runningIds: readonly string[],
): Set<string> {
  const running = new Set(runningIds)
  return new Set([...dismissedIds].filter(id => running.has(id)))
}

export function toProgressItem(task: Record<string, unknown>): DownloadProgressItem | null {
  const id = String(task.id || task.task_id || '')
  if (!id) return null
  const available = Array.isArray(task.available_actions)
    ? task.available_actions.map(value => String(value))
    : undefined
  return {
    id,
    title: String(task.title || ''),
    filename: String(task.filename || task.title || id),
    status: String(task.status || ''),
    progress_percent: getDisplayedProgress(task),
    downloaded_bytes: Number(task.downloaded_bytes || 0),
    total_bytes: Number(task.total_bytes || 0),
    speed_bytes_per_sec: Number(task.speed_bytes_per_sec || 0),
    eta_seconds: Number(task.eta_seconds || 0),
    ...(available ? { available_actions: available } : {}),
    is_live: Boolean(task.is_live),
  }
}

export function toCompleteItem(task: Record<string, unknown>): DownloadCompleteItem | null {
  const id = String(task.id || task.task_id || '')
  if (!id) return null
  return {
    id,
    title: String(task.title || ''),
    filename: String(task.filename || task.title || id),
    output_path: String(task.output_path || ''),
    downloaded_bytes: Number(task.downloaded_bytes || task.total_bytes || 0),
    output_is_file: task.output_is_file !== false,
  }
}

export function enqueueCompleteItem(
  queue: DownloadCompleteItem[],
  item: DownloadCompleteItem,
): DownloadCompleteItem[] {
  if (!item?.id) return queue
  if (queue.some(existing => existing.id === item.id)) {
    return queue.map(existing => existing.id === item.id ? item : existing)
  }
  return [...queue, item].slice(-COMPLETE_QUEUE_CAP)
}

export function dismissCompleteItem(queue: DownloadCompleteItem[], id: string): DownloadCompleteItem[] {
  if (!id) return queue
  return queue.filter(item => item.id !== id)
}

export const EXECUTABLE_OPEN_RE = /\.(?:bat|cmd|com|exe|js|msi|ps1|scr|vbs)$/i

export function needsExecutableConfirm(path: string): boolean {
  return EXECUTABLE_OPEN_RE.test(path || '')
}
