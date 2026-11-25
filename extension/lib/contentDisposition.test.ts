import { describe, expect, it } from 'vitest'
import { contentDispositionFilename } from './contentDisposition'

describe('Content-Disposition filename parsing', () => {
  it('prefers RFC 5987 names and decodes international filenames', () => {
    expect(contentDispositionFilename("attachment; filename*=UTF-8''%E4%B8%8B%E8%BD%BD%3B%E6%B5%8B%E8%AF%95.iso"))
      .toBe('下载;测试.iso')
    expect(contentDispositionFilename("attachment; filename*=ISO-8859-1''caf%E9.pdf")).toBe('café.pdf')
  })

  it('falls back when the extended filename cleans to empty', () => {
    expect(contentDispositionFilename("attachment; filename*=UTF-8''%00; filename=archive.zip"))
      .toBe('archive.zip')
  })

  it('falls back when filename* declares an unsupported charset', () => {
    expect(contentDispositionFilename("attachment; filename*=UTF-16''%FF%FEa%00.zip; filename=archive.zip"))
      .toBe('archive.zip')
  })

  it('falls back when filename* contains invalid bytes for its charset', () => {
    expect(contentDispositionFilename("attachment; filename*=UTF-8''bad%FF.zip; filename=archive.zip"))
      .toBe('archive.zip')
  })

  it('keeps a semicolon inside a quoted legacy filename', () => {
    expect(contentDispositionFilename('attachment; filename="archive; final.zip"')).toBe('archive; final.zip')
  })

  it('does not cut a surrogate pair at the display-length boundary', () => {
    const prefix = 'a'.repeat(511)
    const parsed = contentDispositionFilename(`attachment; filename="${prefix}😀.zip"`)
    expect(parsed).toBe(prefix)
    expect(parsed.charCodeAt(parsed.length - 1)).not.toBeGreaterThanOrEqual(0xd800)
  })

  it('decodes RFC 2047 Base64 and Quoted-Printable encoded-word filenames', () => {
    // Base64 UTF-8: "中文测试.pdf"
    expect(contentDispositionFilename('attachment; filename="=?UTF-8?B?5Lit5paH5rWL6K+VLnBkZg==?="'))
      .toBe('中文测试.pdf')
    // Multi-word Quoted-Printable UTF-8 with underscore space
    expect(contentDispositionFilename('attachment; filename="=?UTF-8?Q?=E4=B8=AD?= =?UTF-8?Q?=E6=96=87_demo.mp4?="'))
      .toBe('中文 demo.mp4')
  })

  it('recovers raw UTF-8 bytes exposed as Latin-1 in legacy filename= without breaking ISO-8859-1', () => {
    // Raw UTF-8 bytes for "视频下载.mp4" interpreted byte-by-byte as Latin-1
    const rawUtf8AsLatin1 = Array.from(new TextEncoder().encode('视频下载.mp4'), b => String.fromCharCode(b)).join('')
    expect(contentDispositionFilename(`attachment; filename="${rawUtf8AsLatin1}"`)).toBe('视频下载.mp4')
    // Valid ISO-8859-1 single-byte character (0xE9 = é) stays intact
    expect(contentDispositionFilename('attachment; filename="caf\u00e9.pdf"')).toBe('café.pdf')
  })
})
