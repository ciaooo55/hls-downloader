import { describe, expect, it } from 'vitest'
import { filterHarvestLinks, harvestFilterCounts, type HarvestLink } from './pageHarvest'

const sample: HarvestLink[] = [
  { url: 'https://cdn.example.test/a.mp4', filename: 'a.mp4', label: 'Film', extension: 'mp4', category: 'video', source: 'href' },
  { url: 'https://cdn.example.test/b.zip', filename: 'b.zip', label: 'Zip', extension: 'zip', category: 'archive', source: 'href' },
  { url: 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567', filename: 'seed', label: 'seed', extension: '', category: 'torrent', source: 'href' },
]

describe('filterHarvestLinks', () => {
  it('keeps the full confirm list until a category is chosen', () => {
    expect(filterHarvestLinks(sample, 'all')).toHaveLength(3)
    expect(filterHarvestLinks(sample, 'video').map(item => item.filename)).toEqual(['a.mp4'])
  })
})

describe('harvestFilterCounts', () => {
  it('counts visible categories for the filter chips', () => {
    expect(harvestFilterCounts(sample)).toMatchObject({ all: 3, video: 1, archive: 1, torrent: 1 })
  })
})
import { applyHarvestProbes, filterHarvestLinksByMinSize } from './pageHarvest'

describe('harvest size probe helpers', () => {
  const links: HarvestLink[] = [
    { url: 'https://cdn.example.test/a.mp4', filename: 'a.mp4', label: 'A', extension: 'mp4', category: 'video', source: 'href' },
    { url: 'https://cdn.example.test/b.zip', filename: 'b.zip', label: 'B', extension: 'zip', category: 'archive', source: 'href' },
  ]

  it('merges successful sizes without dropping unprobed links', () => {
    const merged = applyHarvestProbes(links, [{ url: links[0].url, size: 8_000_000, ok: true }])
    expect(merged[0].size).toBe(8_000_000)
    expect(merged[1].size).toBeUndefined()
  })

  it('can hide small files after the user asks for a size filter', () => {
    const merged = applyHarvestProbes(links, [
      { url: links[0].url, size: 8_000_000, ok: true },
      { url: links[1].url, size: 12_000, ok: true },
    ])
    expect(filterHarvestLinksByMinSize(merged, 1_000_000).map(item => item.filename)).toEqual(['a.mp4'])
  })
})
