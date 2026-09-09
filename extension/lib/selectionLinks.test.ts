import { describe, expect, it } from 'vitest'
import { selectedDownloadUrls } from './selectionLinks'

describe('selected download links', () => {
  it('combines selected anchors and plain-text URLs without duplicates', () => {
    expect(selectedDownloadUrls(
      ['/one.zip', 'https://cdn.test/two.mp4'],
      'mirror: https://site.test/one.zip, magnet:?xt=urn:btih:ABC123.',
      'https://site.test/watch/page',
    )).toEqual([
      'https://site.test/one.zip',
      'https://cdn.test/two.mp4',
      'magnet:?xt=urn:btih:ABC123',
    ])
  })

  it('rejects script, data and malformed selections', () => {
    expect(selectedDownloadUrls(
      ['javascript:alert(1)', 'data:text/plain,hello', '://broken'],
      'nothing downloadable here',
      'https://site.test/page',
    )).toEqual([])
  })
})
