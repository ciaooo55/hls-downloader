export const QUEUE_DRAG_MIME = 'application/x-hls-queue-task'

export function isQueuedTask(task: { status?: string } | null | undefined): boolean {
  return task?.status === 'queued'
}

export function isQueueReorderDrag(types: ArrayLike<string> | null | undefined): boolean {
  return Array.from(types || []).includes(QUEUE_DRAG_MIME)
}

export function queueDropPlacement(clientY: number, top: number, height: number): 'before' | 'after' {
  return clientY < top + Math.max(height, 1) / 2 ? 'before' : 'after'
}

export function queueReorderDirection(
  sourceId: string,
  targetId: string,
  placement: 'before' | 'after',
): string | null {
  if (!sourceId || !targetId || sourceId === targetId) return null
  return placement + ':' + targetId
}

export function applyQueueReorder<T extends { id: string; status?: string; queue_position?: number }>(
  tasks: T[],
  sourceId: string,
  targetId: string,
  placement: 'before' | 'after',
): T[] {
  if (!queueReorderDirection(sourceId, targetId, placement)) return tasks
  const queued = tasks.filter(isQueuedTask)
  const source = queued.find(task => task.id === sourceId)
  const target = queued.find(task => task.id === targetId)
  if (!source || !target) return tasks
  const nextQueued = queued.filter(task => task.id !== sourceId)
  const index = nextQueued.findIndex(task => task.id === targetId)
  if (index < 0) return tasks
  nextQueued.splice(placement === 'before' ? index : index + 1, 0, source)
  const positions = new Map(nextQueued.map((task, rank) => [task.id, rank + 1]))
  return tasks.map(task => {
    const position = positions.get(task.id)
    return position ? { ...task, queue_position: position } : task
  })
}
