import { describe, expect, it } from 'vitest'
import {
  cookieLookupUrl,
  cookiePermissionAllows,
  normalizeCookiePermissionHosts,
} from './browserCookies'

describe('cookieLookupUrl', () => {
  it('accepts http and https URLs', () => {
    expect(cookieLookupUrl('https://site.test/watch')).toBe('https://site.test/watch')
    expect(cookieLookupUrl('http://site.test/file')).toBe('http://site.test/file')
  })

  it('rejects URLs that browser.cookies cannot query', () => {
    expect(cookieLookupUrl('magnet:?xt=urn:btih:abc')).toBe('')
    expect(cookieLookupUrl('blob:https://site.test/id')).toBe('')
    expect(cookieLookupUrl('about:blank')).toBe('')
    expect(cookieLookupUrl('')).toBe('')
  })
})

describe('cookie site permissions', () => {
  it('is denied by default and allows only an explicitly authorized site', () => {
    expect(cookiePermissionAllows(
      'https://cdn.test/video.m3u8',
      'https://www.site.test/watch',
      [],
    )).toBe(false)
    expect(cookiePermissionAllows(
      'https://cdn.test/video.m3u8',
      'https://www.site.test/watch',
      ['site.test'],
    )).toBe(true)
    expect(cookiePermissionAllows(
      'https://cdn.test/video.m3u8',
      'https://other.test/watch',
      ['site.test'],
    )).toBe(false)
  })

  it('normalizes, deduplicates, and bounds persisted hosts', () => {
    expect(normalizeCookiePermissionHosts([
      'WWW.Site.Test:443',
      'https://site.test/watch',
      '',
    ])).toEqual(['site.test'])
    expect(normalizeCookiePermissionHosts(
      Array.from({ length: 300 }, (_, index) => `site-${index}.test`),
    )).toHaveLength(256)
  })
})
