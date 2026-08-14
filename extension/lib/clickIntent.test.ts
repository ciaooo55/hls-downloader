import { describe, expect, it } from 'vitest'
import { isAuthenticationNavigation, isLikelyDownloadControl, isLikelyDownloadUrl, resolveDownloadTarget, shouldTrackDownloadIntent } from './clickIntent'
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

  it('tracks only download-specific normal links and generated controls', () => {
    expect(shouldTrackDownloadIntent({ hintedHref: 'https://cdn.test/file.zip' })).toBe(true)
    expect(shouldTrackDownloadIntent({ directHref: 'https://site.test/download?id=7' })).toBe(true)
    expect(shouldTrackDownloadIntent({ directHref: 'https://site.test/account/settings' })).toBe(false)
    expect(shouldTrackDownloadIntent({ hints: ['下载资源', 'javascript-link'] })).toBe(true)
    expect(shouldTrackDownloadIntent({ hints: ['展开详情', 'javascript-link'] })).toBe(false)
    expect(shouldTrackDownloadIntent({ ctrlForce: true })).toBe(true)
  })

  it('recognizes signed attachment gateways and localized download controls', () => {
    expect(isLikelyDownloadUrl('https://app.test/backend/content?id=42&fn=project.zip&cd=attachment')).toBe(true)
    expect(isLikelyDownloadControl(['Télécharger'])).toBe(true)
    expect(isLikelyDownloadControl(['ダウンロード'])).toBe(true)
  })

  it('never records Google or third-party OAuth navigation as a download intent', () => {
    const google = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=app&redirect_uri=https%3A%2F%2Fsite.test%2Fcallback&response_type=code&scope=profile'
    const microsoft = 'https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=app&redirect_uri=https%3A%2F%2Fsite.test%2Fcallback&response_type=code'
    const github = 'https://github.com/login/oauth/authorize?client_id=app&redirect_uri=https%3A%2F%2Fsite.test%2Fcallback'
    for (const url of [google, microsoft, github]) {
      expect(isAuthenticationNavigation(url)).toBe(true)
      expect(shouldTrackDownloadIntent({ directHref: url, ctrlForce: true, hints: ['下载'] })).toBe(false)
    }
  })

  it('keeps real download gateways eligible without treating every route as a file', () => {
    expect(isLikelyDownloadUrl('https://files.test/attachments/report?id=1')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/firmware.bin')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/lib.jar')).toBe(true)
    expect(isLikelyDownloadUrl('https://site.test/watch/episode-1')).toBe(false)
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
