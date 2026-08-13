import { describe, expect, it } from 'vitest'
import { formatTaskExport, parseUrlList, URL_LIST_LIMIT } from './urlList'

describe('parseUrlList', () => {
  it('extracts http, https, ftp, sftp and magnet links from messy text', () => {
    const text = [
      '# comment',
      'see https://cdn.example.test/a.mp4, please',
      'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567',
      'ftp://files.example.test/keep.bin',
      'sftp://files.example.test/keep.bin',
      '<a href="https://cdn.example.test/b.m3u8">playlist</a>',
      'https://cdn.example.test/a.mp4',
    ].join('\n')
    expect(parseUrlList(text).urls).toEqual([
      'https://cdn.example.test/b.m3u8',
      'https://cdn.example.test/a.mp4',
      'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567',
      'ftp://files.example.test/keep.bin',
      'sftp://files.example.test/keep.bin',
    ])
  })

  it('ignores javascript and incomplete magnets', () => {
    expect(parseUrlList('javascript:alert(1) magnet:?dn=nohash').urls).toEqual([])
  })

  it('caps the list at the batch API limit', () => {
    const text = Array.from({ length: URL_LIST_LIMIT + 5 }, (_, index) => `https://cdn.example.test/${index}.bin`).join('\n')
    const parsed = parseUrlList(text)
    expect(parsed.urls).toHaveLength(URL_LIST_LIMIT)
    expect(parsed.truncated).toBe(true)
  })
})

describe('formatTaskExport', () => {
  it('writes comments that parseUrlList can ignore on re-import', () => {
    const exported = formatTaskExport([
      { url: 'https://cdn.example.test/a.mp4', filename: 'a.mp4' },
      { url: 'https://cdn.example.test/b.bin', title: 'other' },
    ])
    expect(exported).toContain('# a.mp4')
    expect(parseUrlList(exported).urls).toEqual([
      'https://cdn.example.test/a.mp4',
      'https://cdn.example.test/b.bin',
    ])
  })
})
