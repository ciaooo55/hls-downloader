export const SPEED_CHART_LIMIT = 180

export type SpeedChartModel = {
  line: string
  area: string
  peak: number
  average: number
  current: number
  width: number
  height: number
}

export function normalizeSpeedSamples(values: unknown, limit = SPEED_CHART_LIMIT): number[] {
  if (!Array.isArray(values)) return []
  const samples: number[] = []
  for (const value of values.slice(-limit)) {
    const speed = Math.max(0, Math.round(Number(value) || 0))
    samples.push(Number.isFinite(speed) ? speed : 0)
  }
  return samples
}

export function buildSpeedChart(
  values: unknown,
  current = 0,
  peakHint = 0,
  width = 120,
  height = 36,
): SpeedChartModel | null {
  const samples = normalizeSpeedSamples(values)
  if (samples.length < 2) return null
  const peak = Math.max(...samples, Math.max(0, Number(peakHint) || 0), 1)
  const average = samples.reduce((sum, value) => sum + value, 0) / samples.length
  const points = samples.map((value, index) => {
    const x = (index / (samples.length - 1)) * width
    const y = height - 1 - (value / peak) * (height - 2)
    return `${x.toFixed(2)},${y.toFixed(2)}`
  })
  return {
    line: points.join(' '),
    area: `0,${height} ${points.join(' ')} ${width},${height}`,
    peak,
    average,
    current: Math.max(0, Number(current) || 0),
    width,
    height,
  }
}
