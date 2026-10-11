import { describe, expect, it } from 'vitest'
import {
  cookieLookupUrl,
  cookieHeaderForUrl,
  cookiePermissionAllows,
  normalizeCookiePermissionHosts,
  replayCookieIdentity,
  cookiePartitionSite,
  cookiePartitionKeyForSrcdoc,
  isCookieHostPermissionError,
  isSrcdocCookieHostPermissionError,
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
  it('uses the schemeful top-level site for subdomains, ports and private suffixes', () => {
    expect(cookiePartitionSite('https://www.example.co.uk:8443/watch')).toBe('https://example.co.uk')
    expect(cookiePartitionSite('https://video.alice.github.io/watch')).toBe('https://alice.github.io')
    expect(cookiePartitionSite('https://video.bob.github.io/watch')).toBe('https://bob.github.io')
    expect(cookiePartitionSite('http://127.0.0.1:8080/watch')).toBe('http://127.0.0.1')
    expect(cookiePartitionSite('http://localhost:8080/watch')).toBe('http://localhost')
    expect(cookiePartitionSite('https://[::1]:8080/watch')).toBe('https://[::1]')
    expect(cookiePartitionSite('blob:https://site.test/id')).toBe('')
  })
  it('preserves same-name ordinary and partitioned credentials while merging repeated queries', () => {
    const base = { name: 'session', domain: 'cdn.test', path: '/', hostOnly: true, secure: true }
    const ordinary = { ...base, value: 'ordinary' }
    const partitioned = { ...base, value: 'partitioned', partitionKey: {
      topLevelSite: 'https://site.test', hasCrossSiteAncestor: true,
    } }
    const records = new Map([ordinary, partitioned, ordinary, partitioned]
      .map(cookie => [replayCookieIdentity(cookie), cookie]))
    expect(records.size).toBe(2)
    expect(cookieHeaderForUrl([...records.values()], 'https://cdn.test/chunks/1.ts'))
      .toBe('session=ordinary; session=partitioned')
    expect(replayCookieIdentity(partitioned)).not.toBe(replayCookieIdentity({
      ...partitioned, partitionKey: { ...partitioned.partitionKey, hasCrossSiteAncestor: false },
    }))
  })
  it('selects path cookies without mixing page, sibling, expired or secure cookies', () => {
    const base = { domain: 'cdn.test', path: '/', hostOnly: true, secure: false }
    const cookies = [
      { ...base, name: 'session', value: 'root' },
      { ...base, name: 'session', value: 'chunks', path: '/chunks' },
      { ...base, name: 'page', value: 'secret', path: '/watch' },
      { ...base, name: 'expired', value: 'gone', expirationDate: 1 },
      { ...base, name: 'secure', value: 'tls', secure: true },
      { ...base, name: 'foreign', value: 'no', domain: 'other.test' },
    ]
    expect(cookieHeaderForUrl(cookies, 'http://cdn.test/chunks/1.ts')).toBe('session=chunks; session=root')
    expect(cookieHeaderForUrl(cookies, 'http://cdn.test/chunksevil/1.ts')).toBe('session=root')
    expect(cookieHeaderForUrl(cookies, 'https://cdn.test/chunks/1.ts')).toBe('session=chunks; session=root; secure=tls')
  })
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

  it('normalizes wildcard host rules before matching subdomains', () => {
    expect(normalizeCookiePermissionHosts([
      '*.site.test',
      'https://*.site.test/watch',
    ])).toEqual(['site.test'])
    expect(cookiePermissionAllows(
      'https://cdn.site.test/video.m3u8',
      'https://other.test/watch',
      ['*.site.test'],
    )).toBe(true)
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

describe('srcdoc cookie partition context', () => {
  it('only treats the explicit about:srcdoc host-permission error as recoverable', () => {
    expect(isSrcdocCookieHostPermissionError(new Error('No host permissions for cookies at url: "about:srcdoc".'))).toBe(true)
    expect(isSrcdocCookieHostPermissionError(new Error('No host permissions for cookies at url: "https://cdn.test/".'))).toBe(false)
    expect(isSrcdocCookieHostPermissionError(new Error('Unexpected cookies API failure'))).toBe(false)
    expect(isCookieHostPermissionError(new Error('No host permissions for cookies at url: "https://top.test/".'))).toBe(true)
    expect(isCookieHostPermissionError(new Error('Unexpected cookies API failure'))).toBe(false)
  })

  it('derives the ancestor bit from a complete schemeful-site chain', () => {
    expect(cookiePartitionKeyForSrcdoc('https://www.top.test/page', {
      href: 'about:srcdoc',
      origin: 'https://media.top.test',
      ancestorOrigins: ['https://media.top.test', 'https://top.test'],
    })).toEqual({ topLevelSite: 'https://top.test', hasCrossSiteAncestor: false })

    expect(cookiePartitionKeyForSrcdoc('https://top.test/page', {
      href: 'about:srcdoc',
      origin: 'https://embed.other.test',
      ancestorOrigins: ['https://embed.other.test', 'https://top.test'],
    })).toEqual({ topLevelSite: 'https://top.test', hasCrossSiteAncestor: true })
  })

  it('does not guess when the frame is opaque, ancestors are redacted, or the chain is incomplete', () => {
    const context = {
      href: 'about:srcdoc',
      origin: 'https://embed.other.test',
      ancestorOrigins: ['https://embed.other.test', 'https://top.test'],
    }
    expect(cookiePartitionKeyForSrcdoc('https://top.test', { ...context, origin: 'null' })).toBeUndefined()
    expect(cookiePartitionKeyForSrcdoc('https://top.test', {
      ...context, ancestorOrigins: ['https://embed.other.test', 'null'],
    })).toBeUndefined()
    expect(cookiePartitionKeyForSrcdoc('https://top.test', {
      ...context, ancestorOrigins: ['https://embed.other.test'],
    })).toBeUndefined()
    expect(cookiePartitionKeyForSrcdoc('https://top.test', { ...context, href: 'https://embed.other.test/' })).toBeUndefined()
  })
})
