import { describe, expect, it } from 'vitest'
import { BlobSourceStore } from './blobSourceStore'

describe('blob download source correlation', () => {
  it('maps a fetched Blob download to its page-scoped HTTP source', () => {
    const store = new BlobSourceStore(4, 60_000)
    store.remember({
      blobUrl: 'blob:https://app.test/object-1',
      sourceUrl: 'https://app.test/api/export?id=42',
      tabId: 7,
      frameId: 0,
      pageUrl: 'https://app.test/project',
      seenAt: 1_000,
    })

    expect(store.find('blob:https://app.test/object-1', 2_000)).toMatchObject({
      sourceUrl: 'https://app.test/api/export?id=42',
      tabId: 7,
      pageUrl: 'https://app.test/project',
    })
  })

  it('never invents a source for client-generated or expired blobs', () => {
    const store = new BlobSourceStore(4, 1_000)
    store.remember({
      blobUrl: 'blob:https://app.test/object-1',
      sourceUrl: 'data:text/plain,generated',
      tabId: 7,
      frameId: 0,
      pageUrl: 'https://app.test/project',
      seenAt: 1_000,
    })
    expect(store.find('blob:https://app.test/object-1', 1_500)).toBeUndefined()

    store.remember({
      blobUrl: 'blob:https://app.test/object-2',
      sourceUrl: 'https://app.test/api/export?id=43',
      tabId: 7,
      frameId: 0,
      pageUrl: 'https://app.test/project',
      seenAt: 2_000,
    })
    expect(store.find('blob:https://app.test/object-2', 3_001)).toBeUndefined()
  })

  it('clears only mappings owned by the closed tab', () => {
    const store = new BlobSourceStore()
    store.remember({ blobUrl: 'blob:https://a.test/1', sourceUrl: 'https://a.test/1.zip', tabId: 1, frameId: 0, pageUrl: 'https://a.test', seenAt: 1_000 })
    store.remember({ blobUrl: 'blob:https://b.test/2', sourceUrl: 'https://b.test/2.zip', tabId: 2, frameId: 0, pageUrl: 'https://b.test', seenAt: 1_000 })
    store.clearTab(1)
    expect(store.find('blob:https://a.test/1', 2_000)).toBeUndefined()
    expect(store.find('blob:https://b.test/2', 2_000)).toBeDefined()
  })
})
