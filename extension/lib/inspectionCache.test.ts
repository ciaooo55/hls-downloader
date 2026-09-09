import { describe, expect, it } from 'vitest'
import { InspectionCache } from './inspectionCache'

describe('HLS inspection cache', () => {
  it('deduplicates recent inspection but permits retry after expiry or failure', () => {
    const cache = new InspectionCache(100, 10)
    expect(cache.claim('resource', 1_000)).toBe(true)
    expect(cache.claim('resource', 1_050)).toBe(false)
    cache.release('resource')
    expect(cache.claim('resource', 1_060)).toBe(true)
    expect(cache.claim('resource', 1_200)).toBe(true)
  })

  it('releases closed-tab keys and bounds long browsing sessions', () => {
    const cache = new InspectionCache(10_000, 2)
    cache.claim('1:page:a', 1)
    cache.claim('2:page:b', 2)
    cache.releasePrefix('1:')
    expect(cache.claim('1:page:a', 3)).toBe(true)
    cache.claim('3:page:c', 4)
    expect(cache.claim('2:page:b', 5)).toBe(true)
  })

  it('backs off failed probes without making the resource permanently blind', () => {
    const cache = new InspectionCache(10_000, 10)
    expect(cache.claim('live', 1_000)).toBe(true)
    cache.defer('live', 3_000, 1_100)
    expect(cache.claim('live', 4_099)).toBe(false)
    expect(cache.claim('live', 4_100)).toBe(true)
  })
})
