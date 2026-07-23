import { commandState, type TaskLike } from './taskCommands'

export type TaskContextAction =
  | 'details' | 'start' | 'pause' | 'resume' | 'cancel'
  | 'retry' | 'preview' | 'launch' | 'open' | 'log' | 'delete'
  | 'deleteFiles' | 'queue_up' | 'queue_down' | 'queue_top' | 'queue_bottom'

export function taskContextActions(input: TaskLike | TaskLike[]): TaskContextAction[] {
  const tasks = Array.isArray(input) ? input : [input]
  if (!tasks.length) return []
  const task = tasks[0]
  const commands = commandState(tasks)
  const actions: TaskContextAction[] = tasks.length === 1 ? ['details'] : []
  if (tasks.length === 1 && task.available_actions?.includes('preview')) actions.push('preview')
  if (commands.start) actions.push('start')
  if (commands.pause) actions.push('pause')
  if (commands.resume) actions.push('resume')
  if (commands.cancel) actions.push('cancel')
  if (commands.retry) actions.push('retry')
  if (tasks.length === 1) {
    if (task.available_actions?.includes('queue_up')) actions.push('queue_up')
    if (task.available_actions?.includes('queue_top')) actions.push('queue_top')
    if (task.available_actions?.includes('queue_down')) actions.push('queue_down')
    if (task.available_actions?.includes('queue_bottom')) actions.push('queue_bottom')
  }
  if (tasks.length === 1 && task.available_actions?.includes('launch')) actions.push('launch')
  if (commands.open) actions.push('open')
  if (commands.log) actions.push('log')
  if (commands.delete) actions.push('delete')
  const canDeleteFiles = tasks.every(value => value.available_actions
    ? value.available_actions.includes('delete_files')
    : value.status !== 'done' || Boolean(value.output_path))
  if (canDeleteFiles) actions.push('deleteFiles')
  return actions
}
