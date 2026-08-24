import { describe, expect, it } from 'vitest'
import { contextMenuCapabilities } from './contextMenuActions'

describe('browser context menu capabilities', () => {
  it('keeps media actions for verified media and player contexts', () => {
    expect(contextMenuCapabilities({ linkUrl: 'https://cdn.test/master.m3u8' })).toMatchObject({ download: true, media: true })
    expect(contextMenuCapabilities({ srcUrl: 'blob:https://video.test/id', mediaType: 'video' })).toMatchObject({ download: true, media: true })
    expect(contextMenuCapabilities({ srcUrl: 'https://cdn.test/audio', mediaType: 'audio' })).toMatchObject({ download: true, media: true })
  })

  it('removes cast and TVBox actions from ordinary files', () => {
    for (const url of ['https://files.test/setup.exe', 'https://files.test/archive.zip', 'https://files.test/manual.pdf']) {
      expect(contextMenuCapabilities({ linkUrl: url })).toMatchObject({ download: true, media: false })
    }
  })

  it('does not offer actions for unsupported page context', () => {
    expect(contextMenuCapabilities({})).toEqual({ url: '', download: false, media: false })
  })
})
