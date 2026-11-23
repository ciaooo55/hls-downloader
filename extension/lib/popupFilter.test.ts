import { describe, expect, it } from 'vitest'
import { classifyResourceCategory, countPopupCategories, filterPopupResources } from './popupFilter'
import type { MediaResource } from './resources'

function item(overrides: Partial<MediaResource>): MediaResource {
  return {
    id: overrides.id || 'res-1',
    url: overrides.url || 'https://cdn.example/video.mp4',
    kind: overrides.kind || 'media',
    seenAt: 1,
    ...overrides,
  }
}

describe('popup resource filtering', () => {
  const sample: MediaResource[] = [
    item({ id: 'hls', url: 'https://cdn.example/master.m3u8', kind: 'hls', title: 'Live Lecture 1080p', quality: '1080p' }),
    item({ id: 'dash', url: 'https://cdn.example/stream.mpd', kind: 'dash', title: 'Movie Stream' }),
    item({ id: 'mp4', url: 'https://media.example/clip.mp4', kind: 'media', mimeType: 'video/mp4', title: 'Demo Clip' }),
    item({ id: 'audio', url: 'https://audio.example/track.m4a', kind: 'media', mimeType: 'audio/mp4', title: 'Podcast Episode' }),
    item({ id: 'zip', url: 'https://files.example/archive.zip', kind: 'file', filename: 'release-10.zip' }),
  ]

  it('classifies stream, video, audio, and file resources accurately', () => {
    expect(classifyResourceCategory(sample[0])).toBe('stream')
    expect(classifyResourceCategory(sample[1])).toBe('stream')
    expect(classifyResourceCategory(sample[2])).toBe('video')
    expect(classifyResourceCategory(sample[3])).toBe('audio')
    expect(classifyResourceCategory(sample[4])).toBe('file')
  })

  it('counts categories consistently', () => {
    expect(countPopupCategories(sample)).toEqual({
      all: 5,
      stream: 2,
      video: 1,
      audio: 1,
      file: 1,
    })
  })

  it('filters by category and case-insensitive keyword across title, filename, quality, and host', () => {
    expect(filterPopupResources(sample, 'stream', '').map(r => r.id)).toEqual(['hls', 'dash'])
    expect(filterPopupResources(sample, 'audio', '').map(r => r.id)).toEqual(['audio'])
    expect(filterPopupResources(sample, 'all', '1080P').map(r => r.id)).toEqual(['hls'])
    expect(filterPopupResources(sample, 'all', 'files.example').map(r => r.id)).toEqual(['zip'])
    expect(filterPopupResources(sample, 'file', 'release').map(r => r.id)).toEqual(['zip'])
    expect(filterPopupResources(sample, 'video', 'podcast')).toEqual([])
  })

  it('matches variant quality labels and height-derived resolutions in keyword search', () => {
    const withVariants: MediaResource[] = [
      item({
        id: 'adaptive',
        url: 'https://cdn.example/master.m3u8',
        kind: 'hls',
        title: 'Course Recording',
        variants: [
          { url: 'https://cdn.example/720.m3u8', height: 720, bandwidth: 2_500_000 },
          { url: 'https://cdn.example/4k.m3u8', width: 3840, height: 2160, quality: '4K' },
        ],
      }),
      item({
        id: 'plain',
        url: 'https://cdn.example/clip.mp4',
        kind: 'media',
        mimeType: 'video/mp4',
        title: 'Short Clip',
        height: 480,
      }),
    ]
    expect(filterPopupResources(withVariants, 'all', '720p').map(r => r.id)).toEqual(['adaptive'])
    expect(filterPopupResources(withVariants, 'all', '2160p').map(r => r.id)).toEqual(['adaptive'])
    expect(filterPopupResources(withVariants, 'all', '4k').map(r => r.id)).toEqual(['adaptive'])
    expect(filterPopupResources(withVariants, 'all', '3840x2160').map(r => r.id)).toEqual(['adaptive'])
    expect(filterPopupResources(withVariants, 'all', '480p').map(r => r.id)).toEqual(['plain'])
    expect(filterPopupResources(withVariants, 'all', '1080p')).toEqual([])
  })
})
