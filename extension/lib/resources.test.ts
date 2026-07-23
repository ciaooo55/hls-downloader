import { describe, expect, it } from 'vitest'
import { classifyDownload, classifyResource, isGenericMediaName, matchesDownloadClick, mergeResources, pageResourceKey, replayableRequestHeaders, resourceRequestIdentity, shouldTakeover, suggestedResourceFilename } from './resources'

describe('resource rules', () => {
  it('filters HLS segments but retains manifests', () => {
    expect(classifyResource('https://cdn.test/a.m3u8')).toBe('hls')
    expect(classifyResource('https://cdn.test/0001.ts')).toBeNull()
  })
  it('deduplicates resources', () => {
    const item = { id: '1', url: 'https://a.test/v.mp4', kind: 'media' as const, seenAt: Date.now() }
    expect(mergeResources([item], { ...item, size: 20 })).toHaveLength(1)
    expect(mergeResources([item], { ...item, size: 20 })[0].size).toBe(20)
  })
  it('honors Alt bypass and Ctrl force', () => {
    const base = { url: 'https://a.test/file.zip', size: 20, enabled: true, minimumBytes: 10, excludedHosts: [], explicitClick: true }
    expect(shouldTakeover({ ...base, altBypass: true })).toBe(false)
    expect(shouldTakeover({ ...base, enabled: false, ctrlForce: true })).toBe(true)
    expect(shouldTakeover({ ...base, size: 9 })).toBe(false)
    expect(shouldTakeover({ ...base, size: 0 })).toBe(true)
    expect(shouldTakeover({ ...base, url: 'https://sub.blocked.test/file.zip', excludedHosts: ['blocked.test'] })).toBe(false)
  })
  it('takes over unknown-size downloads and classifies response filenames', () => {
    expect(classifyDownload('https://cdn.test/get?id=1', 'application/octet-stream', 'setup.exe')).toBe('file')
    expect(shouldTakeover({
      url: 'https://cdn.test/get?id=1', filename: 'archive.zip', size: -1,
      enabled: true, minimumBytes: 1024 * 1024, excludedHosts: [], explicitClick: true,
    })).toBe(true)
  })
  it('uses a page title when an HLS manifest has a generic filename', () => {
    expect(suggestedResourceFilename({
      kind: 'hls',
      url: 'https://cdn.test/video.m3u8?token=1',
      pageUrl: 'https://site.test/watch/episode-12',
      title: '第十二集：重新出发',
      filename: 'video.m3u8',
    })).toBe('第十二集：重新出发')
    expect(suggestedResourceFilename({
      kind: 'hls',
      url: 'https://cdn.test/series/episode-07.m3u8',
      title: '网页标题',
      filename: 'episode-07.m3u8',
    })).toBe('episode-07')
    expect(isGenericMediaName('1080p HLS 视频流')).toBe(true)
    expect(isGenericMediaName('video_1080p.m3u8')).toBe(true)
    expect(isGenericMediaName('master-high.m3u8')).toBe(true)
    expect(isGenericMediaName('HLS 720p')).toBe(true)
    expect(suggestedResourceFilename({
      kind: 'hls',
      url: 'https://cdn.test/master.m3u8',
      pageUrl: 'https://site.test/watch/real-title?episode=12#player',
      title: '1080p HLS 视频流',
      filename: 'master.m3u8',
    })).toBe('real-title')
  })
  it('isolates captured resources by current page inside the same tab', () => {
    expect(pageResourceKey(9, 'https://site.test/watch/1#player')).toBe(pageResourceKey(9, 'https://site.test/watch/1'))
    expect(pageResourceKey(9, 'https://site.test/watch/1')).not.toBe(pageResourceKey(9, 'https://site.test/watch/2'))
    expect(pageResourceKey(9, 'https://site.test/watch?id=1')).not.toBe(pageResourceKey(9, 'https://site.test/watch?id=2'))
    expect(pageResourceKey(9, 'https://site.test/watch/1')).not.toBe(pageResourceKey(10, 'https://site.test/watch/1'))
  })
  it('replays authentication and browser context without transport-owned headers', () => {
    expect(replayableRequestHeaders({
      Authorization: 'Bearer signed-token',
      'Sec-CH-UA': '"Chromium";v="140"',
      'X-Playback-Token': 'abc',
      Cookie: 'private=1',
      Host: 'cdn.test',
      Range: 'bytes=0-1',
      'Accept-Encoding': 'gzip, br',
    })).toEqual({
      authorization: 'Bearer signed-token',
      'sec-ch-ua': '"Chromium";v="140"',
      'x-playback-token': 'abc',
    })
  })
  it('never invents an Origin that the browser did not send', () => {
    expect(resourceRequestIdentity({
      pageUrl: 'https://page.test/watch/1',
      requestHeaders: { Referer: 'https://page.test/watch/1', 'User-Agent': 'Browser UA' },
    }, 'Fallback UA')).toEqual({
      referer: 'https://page.test/watch/1',
      origin: '',
      userAgent: 'Browser UA',
    })
    expect(resourceRequestIdentity({
      pageUrl: 'https://page.test/watch/1',
      requestHeaders: { Origin: 'https://page.test' },
    }, 'Fallback UA')).toEqual({
      referer: 'https://page.test/watch/1',
      origin: 'https://page.test',
      userAgent: 'Fallback UA',
    })
  })
  it('requires and matches a recent explicit click', () => {
    const base = { url: 'https://cdn.test/file.zip', size: 2048, enabled: true, minimumBytes: 1024, excludedHosts: [] }
    expect(shouldTakeover(base)).toBe(false)
    expect(shouldTakeover({ ...base, explicitClick: true })).toBe(true)
    expect(shouldTakeover({
      ...base,
      url: 'https://cdn.test/get?id=unknown',
      filename: '',
      explicitClick: true,
    })).toBe(true)
    const intent = {
      href: 'https://cdn.test/start',
      pageUrl: 'https://site.test/download#button',
      altBypass: false,
      ctrlForce: false,
      at: 1000,
    }
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/start',
      finalUrl: 'https://cdn.test/final.zip',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/start',
      referrer: 'https://other.test/download',
    }, 2000)).toBe(false)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/final.zip',
      finalUrl: 'https://cdn.test/mirror.zip',
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, tabId: 8 }, {
      url: 'https://cdn.test/final.zip',
      finalUrl: 'https://cdn.test/mirror.zip',
      chainUrls: ['https://cdn.test/start', 'https://cdn.test/final.zip'],
      referrer: 'https://github.test/redirected-download',
      tabId: 8,
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 8,
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 9,
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, tabId: 8, opensNewTab: true }, {
      url: 'https://cdn.test/start',
      finalUrl: 'https://cdn.test/file.zip',
      tabId: 9,
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true, controlHint: true, tabId: 8 }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
      tabId: 8,
    }, 4500)).toBe(true)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/start',
    }, 9000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: '' }, {
      url: 'https://cdn.test/advert.php',
    }, 2000)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true, tabId: 8 }, {
      url: 'https://cdn.test/unrelated.zip',
      tabId: 8,
    }, 1800)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 2101)).toBe(false)
    expect(matchesDownloadClick({ ...intent, href: 'https://site.test/download#', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick({ ...intent, href: '', generic: true }, {
      url: 'https://cdn.test/generated.zip',
      referrer: 'https://site.test/download',
    }, 3000)).toBe(false)
  })
})
