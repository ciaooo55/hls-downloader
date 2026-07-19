import { commandState, type TaskLike } from './taskCommands'

export type TaskContextAction =
  | 'details' | 'start' | 'pause' | 'resume' | 'cancel'
  | 'retry' | 'preview' | 'launch' | 'open' | 'log' | 'delete'

export function taskContextActions(task: TaskLike): TaskContextAction[] {
  const commands = commandState([task])
  const actions: TaskContextAction[] = ['details']
  if (task.available_actions?.includes('preview')) actions.push('preview')
  if (commands.start) actions.push('start')
  if (commands.pause) actions.push('pause')
  if (commands.resume) actions.push('resume')
  if (commands.cancel) actions.push('cancel')
  if (commands.retry) actions.push('retry')
  if (task.available_actions?.includes('launch')) actions.push('launch')
  if (commands.open) actions.push('open')
  if (commands.log) actions.push('log')
  if (commands.delete) actions.push('delete')
  return actions
}
