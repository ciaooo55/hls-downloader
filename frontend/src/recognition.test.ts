import { describe, expect, it } from 'vitest'
import { recognitionCandidateViews, recognitionView } from './recognition'

describe('recognitionView', () => {
  it('starts one confirmed playlist directly', () => {
    expect(recognitionView({ kind: 'hls', candidates: [{ url: 'https://x/a' }] }).mode).toBe('ready')
  })

  it('asks the user to choose between multiple candidates', () => {
    expect(recognitionView({ kind: 'page', candidates: [{ url: 'a' }, { url: 'b' }] }).mode).toBe('choose')
  })

  it('shows browser extension guidance when static recognition finds nothing', () => {
    const view = recognitionView({ kind: 'none', candidates: [], message: '请使用浏览器插件' })
    expect(view.mode).toBe('not-found')
    expect(view.message).toContain('浏览器插件')
  })
})

describe('recognitionCandidateViews', () => {
  it('turns a signed URL into a concise host, filename and inferred quality', () => {
    const [candidate] = recognitionCandidateViews([{
      url: 'https://video.example.com/live/Movie%20Night_1080p.m3u8?token=very-long-secret',
      source: 'html',
    }])

    expect(candidate.host).toBe('video.example.com')
    expect(candidate.filename).toBe('Movie Night_1080p.m3u8')
    expect(candidate.qualityLabel).toBe('推测 1080p')
    expect(candidate.sourceLabel).toBe('网页内发现')
  })

  it('recommends and moves the highest inferred resolution to the top', () => {
    const candidates = recognitionCandidateViews([
      { url: 'https://cdn.example.com/video/stream_720p.m3u8' },
      { url: 'https://cdn.example.com/video/stream_2160p.m3u8' },
      { url: 'https://cdn.example.com/video/stream_1080p.m3u8' },
    ])

    expect(candidates.map(candidate => candidate.qualityLabel)).toEqual(['推测 2160p', '推测 1080p', '推测 720p'])
    expect(candidates.map(candidate => candidate.recommended)).toEqual([true, false, false])
  })

  it('prefers an adaptive master playlist over an individual rendition', () => {
    const candidates = recognitionCandidateViews([
      { url: 'https://cdn.example.com/hls/video_1080p.m3u8' },
      { url: 'https://cdn.example.com/hls/master.m3u8' },
    ])

    expect(candidates[0].filename).toBe('master.m3u8')
    expect(candidates[0].qualityLabel).toBe('自适应清晰度')
    expect(candidates[0].recommended).toBe(true)
  })

  it('labels a DASH manifest as an adaptive media resource', () => {
    const [candidate] = recognitionCandidateViews([
      { url: 'https://cdn.example.com/video/manifest.mpd', source: 'dash', quality: 'dash' },
    ])

    expect(candidate.filename).toBe('manifest.mpd')
    expect(candidate.qualityLabel).toBe('自适应清晰度')
    expect(candidate.sourceLabel).toBe('DASH 播放清单')
  })

  it('uses optional backend quality metadata while remaining compatible with old responses', () => {
    const candidates = recognitionCandidateViews([
      { url: 'https://cdn.example.com/play?id=high', label: '1080p', quality: '1080p', confidence: 0.97 },
      { url: 'https://cdn.example.com/play?id=auto', label: '主播放清单', quality: 'master', confidence: 0.99 },
      { url: 'https://cdn.example.com/legacy.m3u8' },
    ])

    expect(candidates[0].qualityLabel).toBe('自适应清晰度')
    expect(candidates[0].recommended).toBe(true)
    expect(candidates.find(candidate => candidate.quality === '1080p')?.qualityLabel).toBe('推测 1080p')
    expect(candidates.find(candidate => candidate.filename === 'legacy.m3u8')?.qualityLabel).toBe('清晰度未知')
  })

  it('infers dimensions from quality query parameters but ignores signed tokens', () => {
    const candidates = recognitionCandidateViews([
      { url: 'https://cdn.example.com/play.m3u8?token=1080&resolution=1920x1080' },
      { url: 'https://cdn.example.com/other.m3u8?token=2160' },
    ])

    expect(candidates.find(candidate => candidate.filename === 'play.m3u8')?.qualityLabel).toBe('推测 1080p')
    expect(candidates.find(candidate => candidate.filename === 'other.m3u8')?.qualityLabel).toBe('清晰度未知')
  })

  it('keeps invalid legacy candidate values usable', () => {
    const [candidate] = recognitionCandidateViews([{ url: 'relative/video_720p.m3u8?token=x' }])

    expect(candidate.host).toBe('来源未知')
    expect(candidate.filename).toBe('video_720p.m3u8')
    expect(candidate.qualityLabel).toBe('推测 720p')
  })
})
