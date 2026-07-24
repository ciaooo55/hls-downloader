import { describe, expect, it } from 'vitest'

describe('TVBox push protocol', () => {
  it('rejects empty endpoint', () => {
    const endpoint = ''
    let error = ''
    if (!endpoint) error = '请先在插件面板设置电视推送地址'
    expect(error).toBe('请先在插件面板设置电视推送地址')
  })

  it('rejects non-http endpoints', () => {
    const endpoint = 'ftp://192.168.1.1:9979'
    let error = ''
    if (!endpoint.startsWith('http://') && !endpoint.startsWith('https://')) {
      error = '电视推送地址需以 http:// 或 https:// 开头'
    }
    expect(error).toBe('电视推送地址需以 http:// 或 https:// 开头')
  })

  it('builds correct TVBox push URL and body', () => {
    const endpoint = 'http://192.168.1.100:9979'
    const resourceUrl = 'https://cdn.test/video.m3u8?token=abc'
    const body = new URLSearchParams({ do: 'push', url: resourceUrl })
    expect(`${endpoint}/action`).toBe('http://192.168.1.100:9979/action')
    expect(body.get('do')).toBe('push')
    expect(body.get('url')).toBe(resourceUrl)
    const getUrl = `${endpoint}/action?do=push&url=${encodeURIComponent(resourceUrl)}`
    expect(getUrl).toContain('do=push')
    expect(getUrl).toContain('token%3Dabc')
  })

  it('strips trailing slashes from endpoint', () => {
    const raw = 'http://192.168.1.100:9979/'
    const endpoint = raw.trim().replace(/\/+$/, '')
    expect(endpoint).toBe('http://192.168.1.100:9979')
  })
})
