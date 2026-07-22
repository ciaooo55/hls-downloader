import { describe, expect, it } from 'vitest'
import { isLikelyDownloadControl } from './clickIntent'

describe('download click intent', () => {
  it('accepts explicit download and save controls', () => {
    expect(isLikelyDownloadControl(['下载视频'])).toBe(true)
    expect(isLikelyDownloadControl(['btn downloadButton'])).toBe(true)
    expect(isLikelyDownloadControl(['Export file'])).toBe(true)
    expect(isLikelyDownloadControl(['aria', 'Save as'])).toBe(true)
  })

  it('rejects ordinary page controls', () => {
    expect(isLikelyDownloadControl(['播放', 'play-button'])).toBe(false)
    expect(isLikelyDownloadControl(['展开详情', 'btn primary'])).toBe(false)
    expect(isLikelyDownloadControl(['下一集', 'nextEpisode'])).toBe(false)
    expect(isLikelyDownloadControl(['登录', 'submit'])).toBe(false)
  })
})
