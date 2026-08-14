import { FolderOpen, Pause, Square, Play } from 'lucide-react'
import { fmtBytes, fmtEta, fmtSpeed } from '../format'
import { commandState } from '../taskCommands'
import { taskSizeSummary, taskStatusLabel } from '../taskPresentation'
import type { DownloadProgressItem } from '../downloadOverlay'
import { Button } from './ui'

export default function DownloadProgressPanel({
  tasks,
  busyId,
  onAction,
  onOpenFolder,
}: {
  tasks: DownloadProgressItem[]
  busyId?: string
  onAction: (task: DownloadProgressItem, action: 'pause' | 'resume' | 'cancel') => void
  onOpenFolder?: (task: DownloadProgressItem) => void
}) {
  if (!tasks.length) {
    return (
      <section className="download-progress-empty">
        <strong>没有正在下载的任务</strong>
        <span>新的下载开始后会显示在这里</span>
      </section>
    )
  }

  return (
    <ul className="download-progress-list">
      {tasks.map(task => {
        const commands = commandState([task])
        const percent = Math.max(0, Math.min(100, Number(task.progress_percent) || 0))
        return (
          <li key={task.id} className="download-progress-row">
            <div className="download-progress-copy">
              <strong title={task.filename}>{task.filename || task.title || task.id}</strong>
              <span>{taskStatusLabel(task)} · {fmtSpeed(task.speed_bytes_per_sec)} · 剩余 {fmtEta(task.eta_seconds)}</span>
              <span>{taskSizeSummary(task)} · {percent.toFixed(percent >= 10 ? 0 : 1)}%</span>
            </div>
            <div className="download-progress-bar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(percent)} role="progressbar">
              <i style={{ width: `${percent}%` }} />
            </div>
            <div className="download-progress-actions">
              {commands.pause && (
                <Button type="button" variant="secondary" size="sm" disabled={busyId === task.id} onClick={() => onAction(task, 'pause')}>
                  <Pause size={13} />{commands.pauseLabel}
                </Button>
              )}
              {commands.resume && (
                <Button type="button" variant="secondary" size="sm" disabled={busyId === task.id} onClick={() => onAction(task, 'resume')}>
                  <Play size={13} />{commands.resumeLabel}
                </Button>
              )}
              {commands.cancel && (
                <Button type="button" variant="ghost" size="sm" disabled={busyId === task.id} onClick={() => onAction(task, 'cancel')}>
                  <Square size={12} />取消
                </Button>
              )}
              {onOpenFolder && (
                <Button type="button" variant="ghost" size="sm" onClick={() => onOpenFolder(task)}>
                  <FolderOpen size={13} />位置
                </Button>
              )}
            </div>
          </li>
        )
      })}
    </ul>
  )
}
