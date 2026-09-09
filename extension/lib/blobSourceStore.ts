export interface BlobSourceRecord {
  blobUrl: string
  sourceUrl: string
  tabId: number
  frameId: number
  pageUrl: string
  seenAt: number
}

/**
 * Correlate a browser DownloadItem backed by blob: with the successful HTTP
 * response from which the page created that Blob. Client-generated blobs have
 * no source record and deliberately remain browser-owned.
 */
export class BlobSourceStore {
  private readonly records = new Map<string, BlobSourceRecord>()

  constructor(
    private readonly maxEntries = 128,
    private readonly retentionMs = 60_000,
  ) {}

  remember(value: Omit<BlobSourceRecord, 'seenAt'> & { seenAt?: number }): void {
    if (!/^blob:/i.test(value.blobUrl) || !/^https?:\/\//i.test(value.sourceUrl)) return
    const record: BlobSourceRecord = {
      ...value,
      tabId: Number.isInteger(value.tabId) ? value.tabId : -1,
      frameId: Number.isInteger(value.frameId) ? value.frameId : -1,
      seenAt: Number(value.seenAt) || Date.now(),
    }
    this.records.delete(record.blobUrl)
    this.records.set(record.blobUrl, record)
    this.cleanup(record.seenAt)
    while (this.records.size > this.maxEntries) {
      const oldest = this.records.keys().next().value
      if (!oldest) break
      this.records.delete(oldest)
    }
  }

  find(blobUrl: string, now = Date.now()): BlobSourceRecord | undefined {
    this.cleanup(now)
    const record = this.records.get(blobUrl)
    return record ? { ...record } : undefined
  }

  clearTab(tabId: number): void {
    for (const [blobUrl, record] of this.records) {
      if (record.tabId === tabId) this.records.delete(blobUrl)
    }
  }

  cleanup(now = Date.now()): void {
    for (const [blobUrl, record] of this.records) {
      if (now - record.seenAt > this.retentionMs) this.records.delete(blobUrl)
    }
  }
}
