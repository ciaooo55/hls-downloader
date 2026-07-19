import { describe, expect, it } from 'vitest'
import { classifyResource, mergeResources, shouldTakeover } from './resources'

describe('resource rules', () => {
  it('filters HLS segments but retains manifests', () => {
    expect(classifyResource('https://cdn.test/a.m3u8')).toBe('hls')
    expect(classifyResource('https://cdn.test/0001.ts')).toBeNull()
  })
  it('deduplicates resources', () => {
    const item = { id: '1', url: 'https://a.test/v.mp4', kind: 'media' as const, seenAt: Date.now() }
    expect(mergeResources([item], { ...item, size: 20 })).toHaveLength(1)
    expect(mergeResources([item], { ...item, size: 20 })[0].size).toBe(20)
  })
  it('honors Alt bypass and Ctrl force', () => {
    const base = { url: 'https://a.test/file.zip', size: 20, enabled: true, minimumBytes: 10, excludedHosts: [] }
    expect(shouldTakeover({ ...base, altBypass: true })).toBe(false)
    expect(shouldTakeover({ ...base, enabled: false, ctrlForce: true })).toBe(true)
  })
})
