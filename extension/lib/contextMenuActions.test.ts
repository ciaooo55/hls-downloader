import { describe, expect, it } from 'vitest'
import { canHarvestPageLinks, CONTEXT_MENU_IDS, contextMenuCapabilities, pageDownloadUrls } from './contextMenuActions'

describe('browser context menu capabilities', () => {
  it('keeps media actions for replayable media and player contexts', () => {
    expect(contextMenuCapabilities({ linkUrl: 'https://cdn.test/master.m3u8' })).toMatchObject({ download: true, media: true })
    expect(contextMenuCapabilities({ srcUrl: 'https://cdn.test/audio', mediaType: 'audio' })).toMatchObject({ download: true, media: true })
  })

  it('does not present a page-owned blob as a direct download or cast target', () => {
    expect(contextMenuCapabilities({ srcUrl: 'blob:https://video.test/id', mediaType: 'video' }))
      .toEqual({ url: 'blob:https://video.test/id', download: false, media: false })
  })

  it('does not advertise direct actions for page-local media schemes', () => {
    for (const url of [
      'data:video/mp4;base64,AAAA',
      'mediastream:01234567-89ab-cdef-0123-456789abcdef',
      'filesystem:https://video.test/temporary/movie.mp4',
    ]) {
      expect(contextMenuCapabilities({ srcUrl: url, mediaType: 'video' }))
        .toEqual({ url, download: false, media: false })
    }
  })

  it('keeps magnet links downloadable but never castable', () => {
    expect(contextMenuCapabilities({ linkUrl: 'magnet:?xt=urn:btih:abc' }))
      .toMatchObject({ download: true, media: false })
  })

  it('removes cast and TVBox actions from ordinary files', () => {
    for (const url of ['https://files.test/setup.exe', 'https://files.test/archive.zip', 'https://files.test/manual.pdf']) {
      expect(contextMenuCapabilities({ linkUrl: url })).toMatchObject({ download: true, media: false })
    }
  })

  it('does not offer actions for unsupported page context', () => {
    expect(contextMenuCapabilities({})).toEqual({ url: '', download: false, media: false })
  })

  it('enables page-link harvest only on http(s) pages', () => {
    expect(CONTEXT_MENU_IDS.downloadPageLinks).toBe('hls-download-page-links')
    expect(canHarvestPageLinks('https://files.test/releases')).toBe(true)
    expect(canHarvestPageLinks('http://localhost:8080/index.html')).toBe(true)
    expect(canHarvestPageLinks('chrome://extensions')).toBe(false)
    expect(canHarvestPageLinks('about:blank')).toBe(false)
    expect(canHarvestPageLinks('file:///C:/Downloads/index.html')).toBe(false)
    expect(canHarvestPageLinks('')).toBe(false)
  })

  it('harvests downloadable page anchors and media sources while filtering navigation and unsafe schemes', () => {
    const urls = pageDownloadUrls(
      [
        { href: '/about' },
        { href: '#top' },
        { href: 'javascript:void(0)' },
        { href: 'https://accounts.google.com/o/oauth2/v2/auth?client_id=x&redirect_uri=https://files.test/cb&response_type=code' },
        { href: '/releases/app-1.0.zip' },
        { href: '/releases/app-1.0.zip' },
        { href: '/export?id=99', download: true },
        { href: 'MAGNET:?xt=urn:btih:AbC123' },
        { href: 'https://cdn.test/video/master.m3u8' },
      ],
      [
        'https://cdn.test/video/clip.mp4',
        'blob:https://files.test/local-media',
        'https://files.test/releases',
      ],
      'https://files.test/releases',
    )
    expect(urls).toEqual([
      'https://files.test/releases/app-1.0.zip',
      'https://files.test/export?id=99',
      'magnet:?xt=urn:btih:AbC123',
      'https://cdn.test/video/master.m3u8',
      'https://cdn.test/video/clip.mp4',
    ])
  })

  it('bounds harvested page links to the configured limit', () => {
    const anchors = Array.from({ length: 60 }, (_, index) => ({ href: `/files/item-${index}.zip` }))
    const urls = pageDownloadUrls(anchors, [], 'https://files.test/', 40)
    expect(urls).toHaveLength(40)
    expect(urls[0]).toBe('https://files.test/files/item-0.zip')
    expect(urls[39]).toBe('https://files.test/files/item-39.zip')
  })
})
