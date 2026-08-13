import { describe, expect, it } from 'vitest'
import { emptySiteProfile, headersToLines, linesToHeaders, moveSiteProfile, normalizeSiteProfiles } from './siteProfiles'

describe('site profiles', () => {
  it('drops empty hosts and keeps first-match order', () => {
    const profiles = normalizeSiteProfiles([
      { host: '', cookie: 'x=1' },
      { host: '*.example.test', concurrency: 99, speed_limit_kib: -3, download_dir: ' D:\A ' },
      { host: 'cdn.test', enabled: false, request_headers: { 'X-Token': 'a' } },
    ])
    expect(profiles).toHaveLength(2)
    expect(profiles[0].host).toBe('*.example.test')
    expect(profiles[0].concurrency).toBe(64)
    expect(profiles[0].speed_limit_kib).toBe(0)
    expect(profiles[0].download_dir).toBe('D:\A')
    expect(profiles[1].enabled).toBe(false)
    expect(profiles[1].request_headers).toEqual({ 'X-Token': 'a' })
  })

  it('round-trips extra headers and reorders rules', () => {
    expect(linesToHeaders('X-Token: abc\nReferer: https://a.test/\n')).toEqual({
      'X-Token': 'abc',
      Referer: 'https://a.test/',
    })
    expect(headersToLines({ 'X-Token': 'abc' })).toBe('X-Token: abc')
    const moved = moveSiteProfile([
      { host: 'a.test' },
      { host: 'b.test' },
      { host: 'c.test' },
    ], 2, 0)
    expect(moved.map((item) => item.host)).toEqual(['c.test', 'a.test', 'b.test'])
    expect(emptySiteProfile().enabled).toBe(true)
  })

  it('keeps opt-in proxy fields and drops invalid modes', () => {
    const profiles = normalizeSiteProfiles([
      { host: 'a.test', proxy_mode: 'manual', proxy_url: 'socks5://127.0.0.1:1080' },
      { host: 'b.test', proxy_mode: 'inherit', proxy_url: 'http://127.0.0.1:9' },
    ])
    expect(profiles[0].proxy_mode).toBe('manual')
    expect(profiles[0].proxy_url).toBe('socks5://127.0.0.1:1080')
    expect(profiles[1].proxy_mode).toBe('')
    expect(profiles[1].proxy_url).toBe('')
    expect(emptySiteProfile().proxy_mode).toBe('')
  })
})
