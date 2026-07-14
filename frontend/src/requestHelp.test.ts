import { describe, expect, it } from 'vitest'
import { LEGACY_REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from './requestHelp'

describe('request field guidance', () => {
  it('keeps the previous defaults as examples', () => {
    expect(LEGACY_REQUEST_EXAMPLES).toEqual({
      referer: 'https://missav.ai/',
      origin: 'https://missav.ai',
      userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0',
      cookie: '',
      ffmpegPath: 'bin\\ffmpeg.exe',
    })
  })

  it('explains the required format and when fields may be empty', () => {
    expect(REQUEST_FIELD_HELP.referer).toContain('完整地址')
    expect(REQUEST_FIELD_HELP.origin).toContain('不含路径')
    expect(REQUEST_FIELD_HELP.cookie).toContain('不要填 Cookie:')
    expect(REQUEST_FIELD_HELP.referer).toContain('留空')
    expect(REQUEST_FIELD_HELP.origin).toContain('留空')
  })
})
