import { describe, expect, it } from 'vitest'
import { classifyHarvestCategory, countHarvestCategories, filterHarvestResources } from './harvestLinks'
import type { MediaResource } from './resources'

function item(overrides: Partial<MediaResource>): MediaResource {
  return {
    id: overrides.id || 'res',
    url: overrides.url || 'https://cdn.example/file.bin',
    kind: overrides.kind || 'file',
    seenAt: 1,
    ...overrides,
  }
}

describe('page links harvest categorization and filtering', () => {
  const sample: MediaResource[] = [
    item({ id: 'hls', kind: 'hls', url: 'https://cdn.example/live/master.m3u8', title: '1080P Stream', quality: '1080p' }),
    item({ id: 'mp4', kind: 'media', url: 'https://cdn.example/videos/lecture.mp4', filename: 'lecture.mp4' }),
    item({ id: 'zip', kind: 'file', url: 'https://cdn.example/releases/sdk-v2.zip', filename: 'sdk-v2.zip' }),
    item({ id: 'iso', kind: 'file', url: 'https://mirror.example/debian-13-amd64.iso', filename: 'debian-13-amd64.iso' }),
    item({ id: 'pdf', kind: 'file', url: 'https://docs.example/whitepaper.pdf', filename: 'whitepaper.pdf' }),
  ]

  it('classifies media, archives, installers/images, and documents accurately', () => {
    expect(classifyHarvestCategory(sample[0])).toBe('media')
    expect(classifyHarvestCategory(sample[1])).toBe('media')
    expect(classifyHarvestCategory(sample[2])).toBe('archive')
    expect(classifyHarvestCategory(sample[3])).toBe('installer')
    expect(classifyHarvestCategory(sample[4])).toBe('document')
    expect(countHarvestCategories(sample)).toEqual({
      all: 5,
      media: 2,
      archive: 1,
      installer: 1,
      document: 1,
    })
  })

  it('filters by category and keyword/extension query', () => {
    expect(filterHarvestResources(sample, 'media', '').map(r => r.id)).toEqual(['hls', 'mp4'])
    expect(filterHarvestResources(sample, 'archive', '').map(r => r.id)).toEqual(['zip'])
    expect(filterHarvestResources(sample, 'installer', '').map(r => r.id)).toEqual(['iso'])
    expect(filterHarvestResources(sample, 'document', '').map(r => r.id)).toEqual(['pdf'])
    expect(filterHarvestResources(sample, 'all', '.mp4').map(r => r.id)).toEqual(['mp4'])
    expect(filterHarvestResources(sample, 'all', '1080').map(r => r.id)).toEqual(['hls'])
    expect(filterHarvestResources(sample, 'all', '.zip').map(r => r.id)).toEqual(['zip'])
  })
})
