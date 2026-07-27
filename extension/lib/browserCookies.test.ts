import { describe, expect, it } from 'vitest'
import { cookieLookupUrl } from './browserCookies'

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
