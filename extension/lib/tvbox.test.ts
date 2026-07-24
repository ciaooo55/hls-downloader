import { describe, expect, it } from 'vitest'
import { normalizeTvboxEndpoint, tvboxActionUrl, tvboxPushBody, tvboxPushGetUrl, tvboxResponseError } from './tvbox'

describe('TVBox push protocol', () => {
  it('rejects empty endpoint', () => {
    expect(() => normalizeTvboxEndpoint('')).toThrow('请先在插件面板设置电视推送地址')
  })

  it('rejects non-http endpoints', () => {
    expect(() => normalizeTvboxEndpoint('ftp://192.168.1.1:9979')).toThrow('电视推送地址需以 http:// 或 https:// 开头')
  })

  it('builds correct TVBox push URL and body', () => {
    const endpoint = 'http://192.168.1.100:9979'
    const resourceUrl = 'https://cdn.test/video.m3u8?token=abc'
    const body = new URLSearchParams({ do: 'push', url: resourceUrl })
    expect(normalizeTvboxEndpoint(`${endpoint}/action/`)).toBe(endpoint)
    expect(tvboxActionUrl(endpoint)).toBe(`${endpoint}/action`)
    expect(body.get('do')).toBe('push')
    expect(body.get('url')).toBe(resourceUrl)
    expect(tvboxPushBody(resourceUrl)).toBe(body.toString())
    const getUrl = tvboxPushGetUrl(endpoint, resourceUrl)
    expect(getUrl).toContain('do=push')
    expect(getUrl).toContain('token%3Dabc')
  })

  it('strips trailing slashes from endpoint', () => {
    expect(normalizeTvboxEndpoint('http://192.168.1.100:9979/')).toBe('http://192.168.1.100:9979')
  })

  it('detects explicit TVBox error responses', () => {
    expect(tvboxResponseError('{"ok":false,"message":"推送失败"}')).toBe('推送失败')
    expect(tvboxResponseError('{"ok":true}')).toBe('')
    expect(tvboxResponseError('error: denied')).toBe('error: denied')
  })
})
