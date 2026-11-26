import { describe, expect, it } from 'vitest'
import { isPlayableMediaResource, suggestedResourceFilename } from './resources'

describe('resource filename bounds', () => {
  it('does not split a surrogate pair at the suggested filename limit', () => {
    const prefix = 'a'.repeat(199)
    const result = suggestedResourceFilename({
      kind: 'media',
      url: 'https://cdn.test/movie.mp4',
      filename: `${prefix}😀.mp4`,
    })

    expect(result).toBe(prefix)
    expect(/[\uD800-\uDBFF]$/.test(result)).toBe(false)
  })

  it('prefers sanitized page title + .mp4 over generic Bilibili .m4s or videoplayback names', () => {
    expect(suggestedResourceFilename({
      kind: 'media',
      url: 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30080.m4s?deadline=1',
      audioUrl: 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30280.m4s?deadline=1',
      filename: '100026.m4s',
      title: '【4K】B站演示视频: 第一集',
    })).toBe('【4K】B站演示视频_ 第一集.mp4')

    expect(suggestedResourceFilename({
      kind: 'media',
      url: 'https://rr1.googlevideo.com/videoplayback?mime=video%2Fmp4',
      filename: 'videoplayback',
      title: 'Tech Keynote 2026',
    })).toBe('Tech Keynote 2026.mp4')
  })

  it('classifies playable media vs archives, installers, documents, and magnets', () => {
    expect(isPlayableMediaResource({ kind: 'hls', url: 'https://cdn.test/master.m3u8' })).toBe(true)
    expect(isPlayableMediaResource({ kind: 'dash', url: 'https://cdn.test/manifest.mpd' })).toBe(true)
    expect(isPlayableMediaResource({ kind: 'media', url: 'https://cdn.test/video.mp4' })).toBe(true)
    expect(isPlayableMediaResource({
      kind: 'media',
      url: 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30080.m4s',
      audioUrl: 'https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/1/2/123-1-30280.m4s',
    })).toBe(false)
    expect(isPlayableMediaResource({ kind: 'file', url: 'https://cdn.test/movie.mkv' })).toBe(true)
    expect(isPlayableMediaResource({ kind: 'file', url: 'https://cdn.test/archive.zip' })).toBe(false)
    expect(isPlayableMediaResource({ kind: 'file', url: 'https://cdn.test/installer.iso' })).toBe(false)
    expect(isPlayableMediaResource({ kind: 'file', url: 'https://cdn.test/manual.pdf' })).toBe(false)
    expect(isPlayableMediaResource({ kind: 'magnet', url: 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567' })).toBe(false)
  })
})
