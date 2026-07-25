/**
 * Bounds best-effort manifest inspection in a long-lived MV3 worker. A failed
 * network probe is explicitly released so a renewed signed URL or transient
 * CDN error can be inspected again instead of becoming a permanent blind spot.
 */
export class InspectionCache {
  private readonly entries = new Map<string, number>()

  constructor(private readonly ttlMs = 10 * 60_000, private readonly limit = 800) {}

  claim(key: string, now = Date.now()): boolean {
    this.expire(now)
    if (this.entries.has(key)) return false
    this.entries.set(key, now)
    while (this.entries.size > this.limit) {
      const oldest = this.entries.keys().next().value
      if (oldest === undefined) break
      this.entries.delete(oldest)
    }
    return true
  }

  release(key: string): void {
    this.entries.delete(key)
  }

  releasePrefix(prefix: string): void {
    for (const key of this.entries.keys()) {
      if (key.startsWith(prefix)) this.entries.delete(key)
    }
  }

  private expire(now: number): void {
    for (const [key, claimedAt] of this.entries) {
      if (now - claimedAt < this.ttlMs) break
      this.entries.delete(key)
    }
  }
}
