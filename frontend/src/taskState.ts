export type TaskRecord = Record<string, any> & {
  id?: string
  task_id?: string
  status?: string
  type?: string
}

export function mergeTaskEvent(
  tasks: TaskRecord[],
  event: TaskRecord,
  deletedTaskIds: ReadonlySet<string> = new Set(),
): TaskRecord[] {
  const taskId = event.task_id || event.id
  if (!taskId) return tasks
  if (event.type === 'task_deleted') {
    return tasks.filter(task => task.id !== taskId)
  }
  if (deletedTaskIds.has(taskId)) return tasks
  if (event.type !== 'task_progress' && event.type !== 'task_created') {
    return tasks
  }

  const index = tasks.findIndex(task => task.id === taskId)
  const update = { ...event, id: taskId }
  delete update.type
  delete update.task_id
  if (index < 0) return [update, ...tasks]

  const next = [...tasks]
  next[index] = { ...tasks[index], ...update }
  return next
}

export function mergeTaskEvents(
  tasks: TaskRecord[],
  events: TaskRecord[],
  deletedTaskIds: ReadonlySet<string> = new Set(),
): TaskRecord[] {
  if (!events.length) return tasks
  const updates = new Map<string, TaskRecord>()
  const removed = new Set<string>()
  for (const event of events) {
    const taskId = event.task_id || event.id
    if (!taskId) continue
    if (event.type === 'task_deleted') {
      removed.add(taskId)
      updates.delete(taskId)
      continue
    }
    if (
      deletedTaskIds.has(taskId)
      || (event.type !== 'task_progress' && event.type !== 'task_created')
    ) continue
    const update = { ...(updates.get(taskId) || {}), ...event, id: taskId }
    delete update.type
    delete update.task_id
    updates.set(taskId, update)
  }
  if (!updates.size && !removed.size) return tasks
  const seen = new Set<string>()
  const next: TaskRecord[] = []
  for (const task of tasks) {
    const taskId = String(task.id || '')
    if (removed.has(taskId)) continue
    const update = updates.get(taskId)
    next.push(update ? { ...task, ...update } : task)
    if (update) seen.add(taskId)
  }
  for (const [taskId, update] of updates) {
    if (!seen.has(taskId) && !removed.has(taskId)) next.unshift(update)
  }
  return next
}

export function getDisplayedProgress(task: TaskRecord): number {
  if (task.status === 'done') return 100
  if (task.status === 'merging' || task.status === 'remuxing') {
    return Number(task.post_percent || 0)
  }
  if (Number(task.progress_percent || 0) > 0) {
    return Math.max(0, Math.min(100, Number(task.progress_percent)))
  }
  // A resumed/starting task can carry bytes before the engine publishes a
  // percent; showing 0% next to a non-zero size summary reads as a bug.
  if (Number(task.total_bytes || 0) > 0) {
    return Math.max(0, Math.min(100, (Number(task.downloaded_bytes || 0) * 100) / Number(task.total_bytes)))
  }
  if (!task.total_segments) return 0
  return (Number(task.completed_segments || 0) / Number(task.total_segments)) * 100
}

export function isPausable(task: TaskRecord): boolean {
  return Boolean(
    task.status
    && ['downloading_segments', 'downloading', 'fetching_metadata', 'checking'].includes(task.status),
  )
}

export const ACTIVE_TRANSFER_STATUSES = [
  'downloading',
  'downloading_segments',
  'fetching_metadata',
  'checking',
  'downloading_m3u8',
  'parsing',
] as const

export function isActiveTransfer(status?: string): boolean {
  return Boolean(status && (ACTIVE_TRANSFER_STATUSES as readonly string[]).includes(status))
}

export function isRunningStatus(status?: string): boolean {
  if (!status) return false
  return [
    'downloading',
    'fetching_metadata',
    'checking',
    'downloading_m3u8',
    'downloading_segments',
    'parsing',
    'pausing',
    'merging',
    'remuxing',
  ].includes(status)
}
