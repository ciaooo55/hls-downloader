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

function intent(overrides: Partial<DownloadClickIntent> = {}): DownloadClickIntent {
  return {
    href: 'https://site.test/download?id=7', pageUrl: 'https://site.test/page',
    tabId: 3, frameId: 0, altBypass: false, ctrlForce: false, generic: false,
    opensNewTab: false, controlHint: true, at: 10_000, ...overrides,
  }
}

describe('persistent click-intent store', () => {
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

  it('persists, matches and consumes one same-tab redirect intent exactly once', async () => {
    const storage = new MemoryStorage()
    const store = new ClickIntentStore(storage, 'intents', () => 10_300)
    await store.remember(intent())
    const download = {
      url: 'https://cdn.test/file.zip', referrer: 'https://site.test/page', tabId: 3,
      chainUrls: ['https://site.test/download?id=7', 'https://cdn.test/file.zip'],
    }
    await expect(store.consume(download)).resolves.toMatchObject({ tabId: 3 })
    await expect(store.consume(download)).resolves.toBeUndefined()
  })
})
