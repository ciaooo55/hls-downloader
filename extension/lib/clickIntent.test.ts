import { describe, expect, it } from 'vitest'
import { isLikelyDownloadControl } from './clickIntent'
import { matchesDownloadClick, type DownloadClickIntent } from './resources'

function intent(overrides: Partial<DownloadClickIntent> = {}): DownloadClickIntent {
  return {
    href: '', pageUrl: 'https://site.test/page', tabId: 7, frameId: 0,
    altBypass: false, ctrlForce: false, generic: true, opensNewTab: false,
    controlHint: false, at: 10_000,
    ...overrides,
  }
}

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

  it('never lets a generic click consume an unrelated tab download', () => {
    expect(matchesDownloadClick(intent(), {
      url: 'https://cdn.test/file.zip', referrer: 'https://site.test/page', tabId: 8,
    }, 10_200)).toBe(false)
  })

  it('accepts a redirected download only when its chain contains the clicked URL', () => {
    const clicked = intent({ href: 'https://site.test/download?id=7', generic: false })
    expect(matchesDownloadClick(clicked, {
      url: 'https://cdn.test/file.zip', finalUrl: 'https://cdn.test/file.zip',
      chainUrls: ['https://site.test/download?id=7', 'https://cdn.test/file.zip'],
      referrer: 'https://site.test/page', tabId: 7,
    }, 11_000)).toBe(true)
    expect(matchesDownloadClick(clicked, {
      url: 'https://cdn.test/other.zip', referrer: 'https://site.test/page', tabId: 7,
    }, 11_000)).toBe(false)
  })
})
