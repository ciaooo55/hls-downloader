import { describe, expect, it } from 'vitest'
import { parseCurlCommand } from './curlImport'

describe('parseCurlCommand', () => {
  it('imports browser Copy as cURL headers, cookie and POST body', () => {
    const parsed = parseCurlCommand(`curl 'https://cdn.test/file.mp4?token=1' -H 'Referer: https://site.test/watch' -H 'Origin: https://site.test' -H 'Authorization: Bearer abc' -b 'sid=secret' --data-raw '{"id":1}'`)
    expect(parsed).toMatchObject({
      url: 'https://cdn.test/file.mp4?token=1',
      method: 'POST',
      body: '{"id":1}',
      referer: 'https://site.test/watch',
      origin: 'https://site.test',
      cookie: 'sid=secret',
      headers: { authorization: 'Bearer abc' },
    })
  })

  it('returns null for a regular URL and rejects incomplete curl', () => {
    expect(parseCurlCommand('https://cdn.test/file.mp4')).toBeNull()
    expect(() => parseCurlCommand('curl -H')).toThrow(/缺少参数/)
  })

  it('accepts the Windows cmd caret line continuation', () => {
    const parsed = parseCurlCommand('curl "https://cdn.test/a.mp4" ^\r\n -H "Referer: https://site.test/"')
    expect(parsed?.url).toBe('https://cdn.test/a.mp4')
    expect(parsed?.referer).toBe('https://site.test/')
  })
})
