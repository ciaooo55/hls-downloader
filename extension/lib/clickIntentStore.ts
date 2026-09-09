import { matchesDownloadClick, type DownloadClickIntent } from './resources'

export interface IntentStorageArea {
  get(key: string): Promise<Record<string, unknown>>
  set(items: Record<string, unknown>): Promise<void>
}

export interface IntentDownload {
  url: string
  finalUrl?: string
  referrer?: string
  chainUrls?: string[]
  tabId?: number
}

const RETENTION_MS = 7_000
const MAX_INTENTS = 20

function optionalNumber(value: unknown): number | undefined {
  if (value === undefined || value === null || value === '') return undefined
  const result = Number(value)
  return Number.isFinite(result) ? result : undefined
}

/** Small persistent queue that survives MV3 service-worker suspension. */
export class ClickIntentStore {
  private intents: DownloadClickIntent[] = []
  private hydrated = false
  private hydration: Promise<void> | null = null

  constructor(
    private readonly storage: IntentStorageArea,
    private readonly key = 'click-intents',
    private readonly now: () => number = Date.now,
  ) {}

  hydrate(): Promise<void> {
    if (this.hydrated) return Promise.resolve()
    if (this.hydration) return this.hydration
    this.hydration = (async () => {
      try {
        const stored = await this.storage.get(this.key)
        const values: unknown[] = Array.isArray(stored[this.key]) ? stored[this.key] as unknown[] : []
        const now = this.now()
        const restored = values
          .filter((item: unknown): item is Record<string, unknown> => Boolean(item && typeof item === 'object'))
          .map(item => ({
            href: String(item.href || ''),
            pageUrl: String(item.pageUrl || ''),
            altBypass: Boolean(item.altBypass),
            ctrlForce: Boolean(item.ctrlForce),
            generic: Boolean(item.generic),
            tabId: optionalNumber(item.tabId),
            frameId: optionalNumber(item.frameId),
            opensNewTab: Boolean(item.opensNewTab),
            controlHint: Boolean(item.controlHint),
            at: Number(item.at) || now,
          }))
          .filter(item => now - item.at <= RETENTION_MS)
        const seen = new Set<string>()
        this.intents = [...this.intents, ...restored]
          .sort((left, right) => right.at - left.at)
          .filter(intent => {
            const identity = [
              intent.at, intent.href, intent.pageUrl, intent.tabId ?? '', intent.frameId ?? '',
              intent.generic ? 1 : 0, intent.ctrlForce ? 1 : 0, intent.altBypass ? 1 : 0,
            ].join('|')
            if (seen.has(identity)) return false
            seen.add(identity)
            return true
          })
          .slice(0, MAX_INTENTS)
      } catch {
        // In-memory intent handling remains available when session storage is unavailable.
      } finally {
        this.hydrated = true
        this.hydration = null
      }
    })()
    return this.hydration
  }

  async remember(intent: DownloadClickIntent): Promise<void> {
    await this.hydrate()
    this.intents.unshift(intent)
    await this.persist()
  }

  async consume(download: IntentDownload): Promise<DownloadClickIntent | undefined> {
    await this.hydrate()
    const now = this.now()
    const previousCount = this.intents.length
    this.intents = this.intents.filter(intent => now - intent.at <= RETENTION_MS)
    const index = this.intents.findIndex(intent => matchesDownloadClick(intent, download, now))
    if (index < 0) {
      // Waiting for the content-script message is a hot 50 ms poll. Persist
      // only when pruning changed state, never once per miss.
      if (this.intents.length !== previousCount) void this.persist()
      return undefined
    }
    const [matched] = this.intents.splice(index, 1)
    await this.persist()
    return matched
  }

  private async persist(): Promise<void> {
    const now = this.now()
    this.intents = this.intents
      .filter(intent => now - intent.at <= RETENTION_MS)
      .sort((left, right) => right.at - left.at)
      .slice(0, MAX_INTENTS)
    try {
      await this.storage.set({ [this.key]: this.intents })
    } catch {
      // The live service worker can still consume in-memory intents.
    }
  }
}
