import { describe, expect, it } from 'vitest'
import { RequestChainStore, requestHeader } from './requestChain'

describe('browser cache validators in replay chains', () => {
  it('drops GET cache validators that depend on the browser cache entity', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'cached-media',
      url: 'https://cdn.test/movie.m3u8',
      tabId: 7,
      type: 'xmlhttprequest',
      method: 'GET',
      timeStamp: 1_000,
      requestHeaders: [
        { name: 'If-None-Match', value: '"browser-cache-etag"' },
        { name: 'If-Modified-Since', value: 'Wed, 21 Oct 2015 07:28:00 GMT' },
        { name: 'If-Range', value: '"partial-browser-entity"' },
        { name: 'Authorization', value: 'Bearer replay-me' },
        { name: 'Referer', value: 'https://site.test/watch' },
      ],
    })

    const chain = store.find({ url: 'https://cdn.test/movie.m3u8' }, 1_010, 7)
    expect(requestHeader(chain, 'if-none-match')).toBe('')
    expect(requestHeader(chain, 'if-modified-since')).toBe('')
    expect(requestHeader(chain, 'if-range')).toBe('')
    expect(requestHeader(chain, 'authorization')).toBe('Bearer replay-me')
    expect(requestHeader(chain, 'referer')).toBe('https://site.test/watch')
  })

  it('keeps application preconditions on POST requests', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'conditional-post',
      url: 'https://site.test/export',
      tabId: 7,
      type: 'xmlhttprequest',
      method: 'POST',
      timeStamp: 2_000,
      requestHeaders: [
        { name: 'If-None-Match', value: '*' },
        { name: 'Content-Type', value: 'application/json' },
      ],
    })

    const chain = store.find({ url: 'https://site.test/export' }, 2_010, 7)
    expect(requestHeader(chain, 'if-none-match')).toBe('*')
    expect(requestHeader(chain, 'content-type')).toBe('application/json')
  })
})
