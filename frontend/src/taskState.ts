export type TaskRecord = Record<string, any> & {
  id?: string
  task_id?: string
  status?: string
  type?: string
}

export function mergeTaskEvent(tasks: TaskRecord[], event: TaskRecord): TaskRecord[] {
  const taskId = event.task_id || event.id
  if (!taskId) return tasks
  if (event.type === 'task_deleted') {
    return tasks.filter(task => task.id !== taskId)
  }
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

export function getDisplayedProgress(task: TaskRecord): number {
  if (task.status === 'done') return 100
  if (task.status === 'merging' || task.status === 'remuxing') {
    return Number(task.post_percent || 0)
  }
  if (!task.total_segments) return 0
  return (Number(task.completed_segments || 0) / Number(task.total_segments)) * 100
}

export function isPausable(task: TaskRecord): boolean {
  return task.status === 'downloading_segments'
}

export function isRunningStatus(status: string): boolean {
  return [
    'downloading',
    'downloading_m3u8',
    'downloading_segments',
    'parsing',
    'pausing',
    'merging',
    'remuxing',
  ].includes(status)
}
