import { describe, expect, it } from 'vitest'
import { buildSpeedChart, normalizeSpeedSamples } from './speedChart'

describe('speed chart model', () => {
  it('drops empty or single-point series', () => {
    expect(buildSpeedChart([])).toBeNull()
    expect(buildSpeedChart([128])).toBeNull()
  })

  it('builds a filled path and keeps the peak', () => {
    const model = buildSpeedChart([0, 1024, 2048], 512, 4096, 100, 30)
    expect(model).not.toBeNull()
    expect(model?.peak).toBe(4096)
    expect(model?.average).toBeCloseTo((0 + 1024 + 2048) / 3)
    expect(model?.current).toBe(512)
    expect(model?.line.split(' ')).toHaveLength(3)
    expect(model?.area.startsWith('0,30 ')).toBe(true)
    expect(model?.area.endsWith(' 100,30')).toBe(true)
  })

  it('sanitizes invalid samples', () => {
    expect(normalizeSpeedSamples(['x', -8, 12.4, Number.NaN])).toEqual([0, 0, 12, 0])
  })
})
