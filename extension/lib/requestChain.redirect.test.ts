import { describe, expect, it } from 'vitest'
import { RequestChainStore } from './requestChain'

describe('redirect request-chain success state', () => {
  it('does not inherit a redirect response as success for the unconfirmed target hop', () => {
    const store = new RequestChainStore()
    store.observeRequest({
      requestId: 'redirect-pending',
      url: 'https://site.test/download.php?id=1',
      tabId: 4,
      type: 'main_frame',
      method: 'GET',
      timeStamp: 1_000,
    })
    store.observeRedirect({
      requestId: 'redirect-pending',
      url: 'https://site.test/download.php?id=1',
      redirectUrl: 'https://cdn.test/protected.bin',
      tabId: 4,
      type: 'main_frame',
      statusCode: 302,
      timeStamp: 1_010,
      responseHeaders: [{ name: 'Location', value: 'https://cdn.test/protected.bin' }],
    })

    // Chromium reuses the request id for the redirected request. Until that
    // target receives its own 2xx/3xx response, successfulOnly must not let a
    // previous-hop 302 authorize browser replay for the new URL.
    store.observeRequest({
      requestId: 'redirect-pending',
      url: 'https://cdn.test/protected.bin',
      tabId: 4,
      type: 'main_frame',
      method: 'GET',
      timeStamp: 1_020,
    })

    expect(store.find({ url: 'https://cdn.test/protected.bin' }, 1_030, 4, true)).toBeUndefined()
    expect(store.find({ url: 'https://cdn.test/protected.bin' }, 1_030, 4)?.statusCode).toBe(0)

    store.observeResponse({
      requestId: 'redirect-pending',
      url: 'https://cdn.test/protected.bin',
      tabId: 4,
      type: 'main_frame',
      statusCode: 403,
      timeStamp: 1_040,
    })
    expect(store.find({ url: 'https://cdn.test/protected.bin' }, 1_050, 4, true)).toBeUndefined()
  })
})
