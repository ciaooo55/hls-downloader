import { describe, expect, it, vi } from 'vitest'
import { SubtitlePcm } from './subtitlePcm'
import { waitForSubtitleAudio } from './subtitleAudio'
import { subtitleAllowed, subtitleSettings, visibleSubtitle } from './subtitleSettings'
import { parseHlsManifest } from './hlsManifest'

describe('字幕的实际行为边界', () => {
  it('自动播放权限或媒体请求不结束时启动会失败，成功和失败都不遗留定时器', async () => {
    vi.useFakeTimers()
    try {
      const pending = waitForSubtitleAudio(new Promise<void>(() => undefined))
      const rejection = expect(pending).rejects.toThrow('音频未能启动')
      await vi.runAllTimersAsync(); await rejection
      await waitForSubtitleAudio(Promise.resolve())
      expect(vi.getTimerCount()).toBe(0)
    } finally { vi.useRealTimers() }
  })
  it('全局关闭不能被站点和页面覆盖，页面规则能显式覆盖站点', () => {
    const url = 'https://video.example/watch?v=1#player'
    const settings = subtitleSettings({ enabled: false, sites: { 'video.example': true } })
    const pages = { 'https://video.example/watch?v=1': true }
    expect(subtitleAllowed(settings, pages, url)).toBe(false)
    settings.enabled = true; settings.sites['video.example'] = false
    expect(subtitleAllowed(settings, {}, url)).toBe(false)
    expect(subtitleAllowed(settings, pages, url)).toBe(true)
    pages['https://video.example/watch?v=1'] = false
    expect(subtitleAllowed(settings, pages, url)).toBe(false)
  })
  it('跨音频块的重采样不丢样本，立体声正确混为单声道 PCM16', () => {
    const chunks: Int16Array[] = []
    const pcm = new SubtitlePcm(44100, chunk => chunks.push(chunk))
    for (let offset = 0; offset < 44100; offset += 128) {
      const count = Math.min(128, 44100 - offset)
      pcm.push([new Float32Array(count).fill(1), new Float32Array(count).fill(0)])
    }
    expect(chunks).toHaveLength(4)
    expect(chunks.flatMap(chunk => [...chunk])).toHaveLength(16000)
    expect(chunks.every(chunk => chunk.every(value => Math.abs(value - 16383.5) <= 0.6))).toBe(true)
  })
  it('字幕微调参与快进后的缓存选择，重叠时使用最新语句', () => {
    const cues = [
      { id: 'one', start: 10, end: 13, source: '', translation: '一', final: true },
      { id: 'two', start: 12, end: 15, source: '', translation: '二', final: true },
    ]
    expect(visibleSubtitle(cues, 12.5, 0)?.id).toBe('two')
    expect(visibleSubtitle(cues, 12.5, 1)?.id).toBe('one')
    expect(visibleSubtitle(cues, 9, 0)).toBeUndefined()
    expect(visibleSubtitle(cues, 9, -2)?.id).toBe('one')
  })
  it('HLS 主清单提前发现独立音轨和内嵌音轨，保持访问签名', () => {
    const info = parseHlsManifest('#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="English",LANGUAGE="en",DEFAULT=YES,URI="audio/en.m3u8"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="中文",LANGUAGE="zh"\n#EXT-X-STREAM-INF:BANDWIDTH=100000,AUDIO="a"\nvideo.m3u8', 'https://media.example/master.m3u8?token=secret')
    expect(info.audioTracks).toHaveLength(2)
    expect(info.audioTracks[0]).toMatchObject({ language: 'en', default: true, url: 'https://media.example/audio/en.m3u8?token=secret' })
    expect(info.audioTracks[1].url).toBeUndefined()
  })
})
