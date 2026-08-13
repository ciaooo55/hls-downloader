import { fmtSpeed } from '../format'
import { buildSpeedChart } from '../speedChart'

export default function SpeedChart({
  samples,
  current = 0,
  peak = 0,
  compact = false,
}: {
  samples?: number[]
  current?: number
  peak?: number
  compact?: boolean
}) {
  const model = buildSpeedChart(samples, current, peak, compact ? 72 : 168, compact ? 18 : 44)
  if (!model) return null
  const summary = `当前 ${fmtSpeed(model.current)} · 峰值 ${fmtSpeed(model.peak)} · 平均 ${fmtSpeed(model.average)}`
  return (
    <div className={compact ? 'speed-spark is-compact' : 'speed-spark'} title={summary}>
      <svg viewBox={`0 0 ${model.width} ${model.height}`} preserveAspectRatio="none" aria-label="速度曲线">
        <polygon points={model.area} className="speed-spark-fill" />
        <polyline points={model.line} className="speed-spark-line" fill="none" />
      </svg>
      {!compact && <small>{summary}</small>}
    </div>
  )
}
