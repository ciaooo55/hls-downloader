import { describe, expect, it } from 'vitest'
import { ClickIntentStore, type IntentStorageArea } from './clickIntentStore'
import type { DownloadClickIntent } from './resources'

class MemoryStorage implements IntentStorageArea {
  values: Record<string, unknown> = {}
  gets = 0
  sets = 0
  gate: Promise<void> = Promise.resolve()

  async get(): Promise<Record<string, unknown>> {
    this.gets += 1
    await this.gate
    return this.values
  }

  async set(items: Record<string, unknown>): Promise<void> {
    this.sets += 1
    this.values = { ...this.values, ...items }
  }
}

class FailFirstGetStorage extends MemoryStorage {
  private first = true

  override async get(): Promise<Record<string, unknown>> {
    this.gets += 1
    if (this.first) {
      this.first = false
      throw new Error('session storage waking')
    }
    return this.values
  }
}

class DelayedFirstSetStorage extends MemoryStorage {
  private first = true
  private releaseFirst!: () => void
  private readonly firstGate = new Promise<void>(resolve => { this.releaseFirst = resolve })
  private markFirstStarted!: () => void
  readonly firstStarted = new Promise<void>(resolve => { this.markFirstStarted = resolve })

  allowFirst() { this.releaseFirst() }

  override async set(items: Record<string, unknown>): Promise<void> {
    this.sets += 1
    if (this.first) {
      this.first = false
      this.markFirstStarted()
      await this.firstGate
    }
    this.values = { ...this.values, ...items }
  }
}

function intent(overrides: Partial<DownloadClickIntent> = {}): DownloadClickIntent {
  return {
    href: 'https://site.test/download?id=7', pageUrl: 'https://site.test/page',
    tabId: 3, frameId: 0, altBypass: false, ctrlForce: false, generic: false,
    opensNewTab: false, controlHint: true, at: 10_000, ...overrides,
  }
}

function matchingDownload() {
  return {
    url: 'https://cdn.test/file.zip', referrer: 'https://site.test/page', tabId: 3,
    chainUrls: ['https://site.test/download?id=7', 'https://cdn.test/file.zip'],
  }
}

describe('persistent click-intent store', () => {
  it('does not consume another tab or frame when a shared download has no source', async () => {
    const store = new ClickIntentStore(new MemoryStorage(), 'intents', () => 10_100)
    const href = 'https://cdn.test/shared.zip'
    await store.remember(intent({ href, tabId: 1 }))
    await store.remember(intent({ href, tabId: 2, at: 10_050 }))
    expect(await store.consume({ url: href })).toBeUndefined()
    expect(await store.consume({ url: href, tabId: 1 })).toMatchObject({ tabId: 1 })
    expect(await store.consume({ url: href, tabId: 2 })).toMatchObject({ tabId: 2 })
    await store.remember(intent({ href, tabId: 1, frameId: 0 }))
    await store.remember(intent({ href, tabId: 1, frameId: 4, at: 10_050 }))
    expect(await store.consume({ url: href, tabId: 1 })).toBeUndefined()
    expect(await store.consume({ url: href, tabId: 1, frameId: 0 })).toMatchObject({ frameId: 0 })
    expect(await store.consume({ url: href, tabId: 1, frameId: 4 })).toMatchObject({ frameId: 4 })
  })

  it('reserves early offers for persisted same-origin download links without consuming ordinary or cross-origin clicks', async () => {
    const storage = new MemoryStorage()
    const store = new ClickIntentStore(storage, 'intents', () => 10_100)
    await store.remember(intent({ downloadAttribute: true }))
    const restored = new ClickIntentStore(storage, 'intents', () => 10_100)
    expect(await restored.consume({ ...matchingDownload(), downloadAttributeOnly: true }))
      .toMatchObject({ downloadAttribute: true })

    for (const click of [intent({ altBypass: true }), intent({ href: 'https://cdn.test/file.zip', downloadAttribute: true })]) {
      await restored.remember(click)
      expect(await restored.consume({ ...matchingDownload(), downloadAttributeOnly: true })).toBeUndefined()
      expect(await restored.consume(matchingDownload())).toMatchObject(click)
    }
  })

  it('shares one hydration across concurrent service-worker requests', async () => {
    const storage = new MemoryStorage()
    let release: () => void = () => {}
    storage.gate = new Promise<void>(resolve => { release = resolve })
    const store = new ClickIntentStore(storage, 'intents', () => 10_100)
    const first = store.hydrate()
    const second = store.hydrate()
    expect(storage.gets).toBe(1)
    release()
    await Promise.all([first, second])
    expect(storage.gets).toBe(1)
  })

  it('does not write session storage while a download waits for its click message', async () => {
    const storage = new MemoryStorage()
    const store = new ClickIntentStore(storage, 'intents', () => 10_100)
    for (let index = 0; index < 20; index += 1) {
      await store.consume({ url: 'https://cdn.test/file.zip', tabId: 3 })
    }
    expect(storage.sets).toBe(0)
  })

  it('merges live and durable intents after a transient hydration failure', async () => {
    const storage = new FailFirstGetStorage()
    storage.values.intents = [intent({ at: 9_900, href: 'https://site.test/old-download' })]
    const store = new ClickIntentStore(storage, 'intents', () => 10_100)

    await store.remember(intent({ at: 10_000, href: 'https://site.test/new-download' }))
    expect(storage.sets).toBe(0)
    expect(storage.values.intents).toEqual([
      expect.objectContaining({ href: 'https://site.test/old-download' }),
    ])

    await store.hydrate()
    expect(storage.gets).toBe(2)
    expect(storage.sets).toBe(1)
    expect(storage.values.intents).toEqual([
      expect.objectContaining({ href: 'https://site.test/new-download', at: 10_000 }),
      expect.objectContaining({ href: 'https://site.test/old-download', at: 9_900 }),
    ])
  })

  it('drops future persisted intents before the next durable queue write', async () => {
    const storage = new MemoryStorage()
    storage.values.intents = [intent({ at: 20_000, href: 'https://site.test/future' })]
    const store = new ClickIntentStore(storage, 'intents', () => 10_100)

    await store.hydrate()
    await store.remember(intent({ at: 10_050, href: 'https://site.test/current' }))

    expect(storage.values.intents).toEqual([
      expect.objectContaining({ href: 'https://site.test/current', at: 10_050 }),
    ])
  })

  it('persists, matches and consumes one same-tab redirect intent exactly once', async () => {
    const storage = new MemoryStorage()
    const store = new ClickIntentStore(storage, 'intents', () => 10_300)
    await store.remember(intent())
    const download = matchingDownload()
    await expect(store.consume(download)).resolves.toMatchObject({ tabId: 3 })
    await expect(store.consume(download)).resolves.toBeUndefined()
  })

  it('does not let a slow remember write resurrect an already consumed click', async () => {
    const storage = new DelayedFirstSetStorage()
    const store = new ClickIntentStore(storage, 'intents', () => 10_300)
    const remembering = store.remember(intent())
    await storage.firstStarted

    const consuming = store.consume(matchingDownload())
    await Promise.resolve()
    expect(storage.sets).toBe(1)

    storage.allowFirst()
    await remembering
    await expect(consuming).resolves.toMatchObject({ tabId: 3 })
    expect(storage.values.intents).toEqual([])
  })
})
