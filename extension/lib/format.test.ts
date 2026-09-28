import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { formatBytes, formatDuration } from './format'

describe('shared byte and duration formatting', () => {
  it('climbs the unit ladder the way the overlay already did', () => {
    expect(formatBytes(2.2 * 1024 ** 3)).toBe('2.2 GB')
    expect(formatBytes(999)).toBe('999 B')
    expect(formatBytes(1024)).toBe('1.0 KB')
    expect(formatBytes(150 * 1024)).toBe('150 KB')
    expect(formatBytes(1024 ** 4)).toBe('1.0 TB')
  })

  it('renders durations as mm:ss and only adds the hour when there is one', () => {
    expect(formatDuration(9)).toBe('0:09')
    expect(formatDuration(61)).toBe('1:01')
    expect(formatDuration(3661)).toBe('1:01:01')
  })

  // 浮层的调用点直接传 Number(resource.duration || 0)，所以 0/NaN 必须走到空串，
  // 不能变成 "0:00"——那会在每个没有时长信息的浮层上多出一行废话。
  it('returns an empty string for a duration the overlay could not read', () => {
    expect(formatDuration(0)).toBe('')
    expect(formatDuration(-1)).toBe('')
    expect(formatDuration(Number.NaN)).toBe('')
  })
})

describe('formatter convergence', () => {
  // 三个入口（后台内容面板、媒体浮层、弹窗）过去各抄一份单位阶梯和时长格式。
  // 这里锁住"只剩一份定义"：谁再写出自己的阶梯，这个测试就报警。
  const sources = [
    fileURLToPath(new URL('../entrypoints/content.ts', import.meta.url)),
    fileURLToPath(new URL('../entrypoints/popup/main.ts', import.meta.url)),
    fileURLToPath(new URL('./format.ts', import.meta.url)),
    fileURLToPath(new URL('./mediaOverlay.ts', import.meta.url)),
  ].map(path => ({ path, text: readFileSync(path, 'utf8') }))

  it('has exactly one unit ladder across the whole extension', () => {
    const ladders = sources.filter(source => source.text.includes('value /= 1024'))
    expect(ladders.map(source => source.path.split(/[\\/]/).pop())).toEqual(['format.ts'])
  })

  it('has every consumer import the shared implementation instead of defining one', () => {
    const content = sources.find(source => source.path.endsWith('content.ts'))!.text
    expect(content).toContain("import { formatBytes, formatDuration } from '../lib/format'")
    const popup = sources.find(source => source.path.endsWith('main.ts'))!.text
    expect(popup).toContain("import { formatBytes, formatDuration } from '../../lib/format'")
    const overlay = sources.find(source => source.path.endsWith('mediaOverlay.ts'))!.text
    expect(overlay).toContain("import { formatBytes, formatDuration } from './format'")
  })

  it('no longer keeps the old per-file formatter names', () => {
    for (const source of sources.filter(entry => !entry.path.endsWith('format.ts'))) {
      expect(source.text).not.toContain('formatSize')
    }
  })
})
