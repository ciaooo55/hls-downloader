import { describe, expect, it } from 'vitest'
import { replayablePostRequest, RequestChainStore, requestHeader } from './requestChain'

describe('redirect request-chain success state', () => {
  it('does not inherit response or replay identity for an unconfirmed redirect target', () => {
    const store = new RequestChainStore()
    const body = new TextEncoder().encode('{"asset":"private"}')
    store.observeRequest({
      requestId: 'redirect-pending',
      url: 'https://site.test/download.php?id=1',
      tabId: 4,
      type: 'xmlhttprequest',
      method: 'POST',
      timeStamp: 1_000,
      requestBody: { raw: [{ bytes: body.buffer }] },
    })
    store.observeRequest({
      requestId: 'redirect-pending',
      url: 'https://site.test/download.php?id=1',
      tabId: 4,
      type: 'xmlhttprequest',
      method: 'POST',
      timeStamp: 1_001,
      requestHeaders: [
        { name: 'Authorization', value: 'Bearer source-only' },
        { name: 'Content-Type', value: 'application/json' },
      ],
    })
    expect(requestHeader(store.find({ url: 'https://site.test/download.php?id=1' }, 1_005, 4), 'authorization'))
      .toBe('Bearer source-only')

    store.observeRedirect({
      requestId: 'redirect-pending',
      url: 'https://site.test/download.php?id=1',
      redirectUrl: 'https://cdn.test/protected.bin',
      tabId: 4,
      type: 'xmlhttprequest',
      method: 'POST',
      statusCode: 307,
      timeStamp: 1_010,
      responseHeaders: [{ name: 'Location', value: 'https://cdn.test/protected.bin' }],
    })

    const redirected = store.find({ url: 'https://cdn.test/protected.bin' }, 1_015, 4)
    expect(redirected?.statusCode).toBe(0)
    expect(requestHeader(redirected, 'authorization')).toBe('')
    expect(replayablePostRequest(redirected)).toEqual({})
    expect(store.find({ url: 'https://cdn.test/protected.bin' }, 1_015, 4, true)).toBeUndefined()

    // An explicitly empty onSendHeaders capture is authoritative. It must not
    // resurrect the source origin's Authorization header merely because both
    // redirect hops share a browser request id.
    store.observeRequest({
      requestId: 'redirect-pending',
      url: 'https://cdn.test/protected.bin',
      tabId: 4,
      type: 'xmlhttprequest',
      method: 'GET',
      timeStamp: 1_020,
      requestHeaders: [],
    })
    store.observeResponse({
      requestId: 'redirect-pending',
      url: 'https://cdn.test/protected.bin',
      tabId: 4,
      type: 'xmlhttprequest',
      method: 'GET',
      statusCode: 403,
      timeStamp: 1_040,
    })

    const rejected = store.find({ url: 'https://cdn.test/protected.bin' }, 1_050, 4)
    expect(requestHeader(rejected, 'authorization')).toBe('')
    expect(store.find({ url: 'https://cdn.test/protected.bin' }, 1_050, 4, true)).toBeUndefined()
  })
})
