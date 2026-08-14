import { describe, expect, it } from 'vitest'
import { isAuthenticationNavigation, isLikelyDownloadControl, isLikelyDownloadUrl, isMediaFileUrl, isPlaybackControl, linkOpensNewTab, resolveClickedLinkHref, resolveDownloadTarget, resolveFormDownloadUrl, shouldTrackDownloadIntent } from './clickIntent'
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
    expect(isLikelyDownloadControl(['保存'])).toBe(false)
    expect(isLikelyDownloadControl(['儲存'])).toBe(false)
    expect(isLikelyDownloadControl(['Save'])).toBe(false)
    expect(isLikelyDownloadControl(['收藏', 'save-progress'])).toBe(false)
  })

  it('treats player chrome as playback, not as a download click', () => {
    expect(isPlaybackControl(['播放', 'play-button'])).toBe(true)
    expect(isPlaybackControl(['vjs-big-play-button'])).toBe(true)
    expect(isPlaybackControl(['下一集', 'nextEpisode'])).toBe(true)
    expect(isPlaybackControl(['清晰度', 'quality'])).toBe(true)
    expect(isPlaybackControl(['弹幕', 'danmaku'])).toBe(true)
    expect(isPlaybackControl(['全屏', 'fullscreen'])).toBe(true)
    expect(isPlaybackControl(['点赞', 'like-button'])).toBe(true)
    expect(isPlaybackControl(['分享', 'share'])).toBe(true)
    expect(isPlaybackControl(['下载视频'])).toBe(false)
    expect(isMediaFileUrl('https://cdn.test/film.mp4')).toBe(true)
    expect(isMediaFileUrl('https://cdn.test/file.zip')).toBe(false)
    expect(shouldTrackDownloadIntent({
      hintedHref: 'https://cdn.test/film.mp4',
      hints: ['播放', 'dplayer-play-icon'],
    })).toBe(false)
    expect(shouldTrackDownloadIntent({
      hintedHref: 'https://cdn.test/film.mp4',
    })).toBe(false)
    expect(shouldTrackDownloadIntent({
      hintedHref: 'https://cdn.test/film.mp4',
      hints: ['下载视频'],
    })).toBe(true)
    expect(shouldTrackDownloadIntent({
      directHref: 'https://cdn.test/film.mp4',
      hints: ['播放', 'play-button'],
    })).toBe(false)
    expect(shouldTrackDownloadIntent({
      directHref: 'https://cdn.test/film.mp4',
    })).toBe(true)
    expect(shouldTrackDownloadIntent({
      hintedHref: 'https://cdn.test/file.zip',
    })).toBe(true)
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
    expect(isLikelyDownloadControl(['下載檔案'])).toBe(true)
    expect(isLikelyDownloadControl(['另存为'])).toBe(true)
    expect(isLikelyDownloadControl(['保存到本地'])).toBe(true)
    expect(isLikelyDownloadControl(['儲存檔案'])).toBe(true)
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

  it('recognizes media files, metalink and affirmative download flags', () => {
    expect(isLikelyDownloadUrl('https://cdn.test/film.mp4')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/master.m3u8?token=1')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/stream.mpd')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/show.mkv')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/clip.webm')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/song.mp3')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/track.flac')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/audio.aac')).toBe(true)
    expect(isLikelyDownloadUrl('https://cdn.test/legacy.f4v')).toBe(true)
    expect(isLikelyDownloadUrl('https://mirror.test/pkg.metalink')).toBe(true)
    expect(isLikelyDownloadUrl('https://mirror.test/pkg.meta4')).toBe(true)
    expect(isLikelyDownloadUrl('magnet:?xt=urn:btih:abc')).toBe(true)
    expect(isLikelyDownloadUrl('https://site.test/get?download=1')).toBe(true)
    expect(isLikelyDownloadUrl('https://site.test/get?download=true')).toBe(true)
    expect(isLikelyDownloadUrl('https://site.test/export?export=1')).toBe(true)
    expect(isLikelyDownloadUrl('https://site.test/get?filename=report.pdf')).toBe(true)
    expect(isLikelyDownloadUrl('https://site.test/get?download=movie.mp4')).toBe(true)
  })

  it('does not treat player-page flags or ordinary watch links as downloads', () => {
    expect(isLikelyDownloadUrl('https://site.test/watch/episode-1?download=0')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/watch?v=abc&download=false')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/watch?download=no')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/watch?download=off')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/play/1?download=0')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/watch?file=episode.mp4')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/watch?filename=stream')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/video.php?id=1&fn=1')).toBe(false)
    expect(isLikelyDownloadUrl('https://site.test/watch?src=https://cdn.test/movie.mp4')).toBe(false)
    expect(isLikelyDownloadUrl('https://www.bilibili.com/video/BV1xx411c7mD?download=0')).toBe(false)
    expect(isLikelyDownloadUrl('https://v.youku.com/v_show/id_XNzE=.html?file=episode.mp4')).toBe(false)
    expect(isLikelyDownloadUrl('https://v.qq.com/x/cover/mzc00200/mzc00200.html')).toBe(false)
    expect(isLikelyDownloadUrl('https://www.iqiyi.com/v_1abcde123.html?file=movie.mp4')).toBe(false)
    expect(shouldTrackDownloadIntent({ directHref: 'https://www.bilibili.com/bangumi/play/ep123', hints: ['下一集'] })).toBe(false)
    expect(shouldTrackDownloadIntent({ directHref: 'https://site.test/watch/episode-1?download=0' })).toBe(false)
    expect(shouldTrackDownloadIntent({ directHref: 'https://site.test/watch?file=episode.mp4' })).toBe(false)
    expect(shouldTrackDownloadIntent({ directHref: 'https://site.test/next-episode', hints: ['下一集'] })).toBe(false)
    expect(shouldTrackDownloadIntent({ directHref: 'https://site.test/watch/episode-1', hints: ['播放'] })).toBe(false)
    expect(shouldTrackDownloadIntent({ hints: ['保存'] })).toBe(false)
  })

  it('accepts only downloader-owned schemes from data download targets', () => {
    expect(resolveDownloadTarget('../file.zip', 'https://site.test/watch/page')).toBe('https://site.test/file.zip')
    expect(resolveDownloadTarget('magnet:?xt=urn:btih:abc', 'https://site.test/watch')).toBe('magnet:?xt=urn:btih:abc')
    expect(resolveDownloadTarget('javascript:download()', 'https://site.test/watch')).toBe('')
    expect(resolveDownloadTarget('data:text/plain,nope', 'https://site.test/watch')).toBe('')
    expect(resolveDownloadTarget('', 'https://site.test/watch')).toBe('')
    expect(resolveDownloadTarget('#', 'https://site.test/watch')).toBe('')
  })

  it('resolves SVG, image-map and form-action download targets', () => {
    expect(resolveClickedLinkHref({
      svgHrefAttribute: '../file.zip',
      baseUrl: 'https://site.test/watch/page',
    })).toBe('https://site.test/file.zip')
    expect(resolveClickedLinkHref({
      svgXlinkHref: 'https://cdn.test/app.apk',
      baseUrl: 'https://site.test/watch',
    })).toBe('https://cdn.test/app.apk')
    expect(resolveClickedLinkHref({
      htmlHref: 'https://site.test/file.zip',
      htmlHrefAttribute: 'file.zip',
      baseUrl: 'https://site.test/watch/page',
    })).toBe('https://site.test/file.zip')
    expect(resolveClickedLinkHref({
      htmlHref: 'https://site.test/watch#',
      htmlHrefAttribute: '#',
      baseUrl: 'https://site.test/watch',
    })).toBe('')
    expect(resolveFormDownloadUrl('/preview', '/files/report.pdf', 'https://site.test/page')).toBe('https://site.test/files/report.pdf')
    expect(resolveFormDownloadUrl('/files/app.zip', '', 'https://site.test/page')).toBe('https://site.test/files/app.zip')
    expect(resolveFormDownloadUrl('/login', '', 'https://site.test/page')).toBe('')
    expect(linkOpensNewTab('_blank')).toBe(true)
    expect(linkOpensNewTab('_self')).toBe(false)
  })

  it('never lets a generic click consume an unrelated tab download', () => {
    expect(matchesDownloadClick(intent(), {
      url: 'https://cdn.test/file.zip', referrer: 'https://site.test/page', tabId: 8,
    }, 10_200)).toBe(false)
  })

  it('does not pair a watch-page navigation with a later same-tab file', () => {
    expect(shouldTrackDownloadIntent({
      directHref: 'https://site.test/watch/episode-1?download=0',
      hints: ['播放', 'play-button'],
    })).toBe(false)
    const watchClick = intent({
      href: 'https://site.test/watch/episode-1?download=0',
      generic: false,
      controlHint: false,
    })
    expect(isLikelyDownloadUrl(watchClick.href)).toBe(false)
    expect(matchesDownloadClick(watchClick, {
      url: 'https://cdn.test/unrelated.zip', referrer: 'https://site.test/page', tabId: 7,
    }, 10_400)).toBe(false)
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
