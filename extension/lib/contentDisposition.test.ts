import { describe, expect, it } from 'vitest'
import { contentDispositionFilename } from './contentDisposition'

describe('Content-Disposition filename parsing', () => {
  it('prefers RFC 5987 names and decodes international filenames', () => {
    expect(contentDispositionFilename("attachment; filename*=UTF-8''%E4%B8%8B%E8%BD%BD%3B%E6%B5%8B%E8%AF%95.iso"))
      .toBe('下载;测试.iso')
    expect(contentDispositionFilename("attachment; filename*=ISO-8859-1''caf%E9.pdf")).toBe('café.pdf')
  })

  it('keeps a semicolon inside a quoted legacy filename', () => {
    expect(contentDispositionFilename('attachment; filename="archive; final.zip"')).toBe('archive; final.zip')
  })
})
