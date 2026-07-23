import { describe, expect, it } from 'vitest'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from './requestHelp'

describe('request field guidance', () => {
  it('uses neutral request examples instead of a site-specific identity', () => {
    expect(REQUEST_EXAMPLES).toEqual({
      referer: 'https://example.com/watch/123',
      origin: 'https://example.com',
      userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0',
      cookie: '',
      ffmpegPath: 'bin\\ffmpeg.exe',
    })
  })

  it('explains the required format and when fields may be empty', () => {
    expect(REQUEST_FIELD_HELP.referer).toContain('完整地址')
    expect(REQUEST_FIELD_HELP.origin).toContain('不含路径')
    expect(REQUEST_FIELD_HELP.cookie).toContain('不要带 Cookie:')
    expect(REQUEST_FIELD_HELP.referer).toContain('留空')
    expect(REQUEST_FIELD_HELP.origin).toContain('留空')
    expect(REQUEST_FIELD_HELP.referer).toContain('浏览器插件任务使用实际捕获值')
    expect(REQUEST_FIELD_HELP.origin).toContain('不会凭空生成 Origin')
    expect(REQUEST_FIELD_HELP.concurrency).toContain('默认 12')
    expect(REQUEST_FIELD_HELP.concurrency).toContain('最高 256')
    expect(REQUEST_FIELD_HELP.maxTasks).toContain('当前默认 3')
  })
})
