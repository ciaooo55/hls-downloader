import { describe, expect, it } from 'vitest'
import { isLikelyDownloadControl, resolveDownloadTarget, shouldTrackDownloadIntent } from './clickIntent'
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

  it('tracks JavaScript links only when they carry a concrete or download-specific signal', () => {
    expect(shouldTrackDownloadIntent({ hintedHref: 'https://cdn.test/file.zip' })).toBe(true)
    expect(shouldTrackDownloadIntent({ hints: ['下载资源', 'javascript-link'] })).toBe(true)
    expect(shouldTrackDownloadIntent({ hints: ['展开详情', 'javascript-link'] })).toBe(false)
    expect(shouldTrackDownloadIntent({ ctrlForce: true })).toBe(true)
  })

  it('accepts only downloader-owned schemes from data download targets', () => {
    expect(resolveDownloadTarget('../file.zip', 'https://site.test/watch/page')).toBe('https://site.test/file.zip')
    expect(resolveDownloadTarget('magnet:?xt=urn:btih:abc', 'https://site.test/watch')).toBe('magnet:?xt=urn:btih:abc')
    expect(resolveDownloadTarget('javascript:download()', 'https://site.test/watch')).toBe('')
    expect(resolveDownloadTarget('data:text/plain,nope', 'https://site.test/watch')).toBe('')
  })

  it('never lets a generic click consume an unrelated tab download', () => {
    expect(matchesDownloadClick(intent(), {
      url: 'https://cdn.test/file.zip', referrer: 'https://site.test/page', tabId: 8,
    }, 10_200)).toBe(false)
  })

  it('accepts redirected or generated downloads for the same tab click window', () => {
    const clicked = intent({ href: 'https://site.test/download?id=7', generic: false })
    expect(matchesDownloadClick(clicked, {
      url: 'https://cdn.test/file.zip', finalUrl: 'https://cdn.test/file.zip',
      chainUrls: ['https://site.test/download?id=7', 'https://cdn.test/file.zip'],
      referrer: 'https://site.test/page', tabId: 7,
    }, 11_000)).toBe(true)
    // Final CDN URL may not include the gateway href; same-tab recent click still matches.
    expect(matchesDownloadClick(clicked, {
      url: 'https://cdn.test/other.zip', referrer: 'https://site.test/page', tabId: 7,
    }, 11_000)).toBe(true)
    // Different tab must never inherit the click intent.
    expect(matchesDownloadClick(clicked, {
      url: 'https://cdn.test/other.zip', referrer: 'https://site.test/page', tabId: 8,
    }, 11_000)).toBe(false)
    // Outside the short click window, generated URLs require an exact chain match.
    expect(matchesDownloadClick(clicked, {
      url: 'https://cdn.test/other.zip', referrer: 'https://site.test/page', tabId: 7,
    }, 13_000)).toBe(false)
  })
})
