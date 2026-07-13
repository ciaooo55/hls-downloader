import React from 'react'
import TaskCard from './TaskCard'

interface Props {
  tasks: any[]
  busyTaskIds: Set<string>
  onStart: (id: string) => void
  onPause: (id: string) => void
  onResume: (id: string) => void
  onCancel: (id: string) => void
  onRetry: (id: string) => void
  onDelete: (id: string) => void
  onLog: (id: string) => void
}

export default function TaskList({ tasks, busyTaskIds, onStart, onPause, onResume, onCancel, onRetry, onDelete, onLog }: Props) {
  if (!tasks.length) {
    return <div style={{ textAlign: 'center', color: '#6b7280', padding: 40 }}>暂无任务</div>
  }
  return (
    <div>
      {tasks.map(t => (
        <TaskCard
          key={t.id}
          task={t}
          busy={busyTaskIds.has(t.id)}
          onStart={() => onStart(t.id)}
          onPause={() => onPause(t.id)}
          onResume={() => onResume(t.id)}
          onCancel={() => onCancel(t.id)}
          onRetry={() => onRetry(t.id)}
          onDelete={() => onDelete(t.id)}
          onLog={() => onLog(t.id)}
        />
      ))}
    </div>
  )
}
