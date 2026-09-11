import { describe, expect, it, vi } from 'vitest'

import { PENDING_TAKEOVER_SETTINGS_KEY, TakeoverSettingsSync } from './takeoverSettingsSync'

class MemoryStorage {
  values: Record<string, unknown> = {}

  async get(keys: string | string[]) {
    const list = Array.isArray(keys) ? keys : [keys]
    return Object.fromEntries(list.map(key => [key, this.values[key]]))
  }

  async set(items: Record<string, unknown>) { Object.assign(this.values, items) }
  async remove(key: string) { delete this.values[key] }
}

class DelayedRemoveStorage extends MemoryStorage {
  private releaseRemove!: () => void
  private readonly removeGate = new Promise<void>(resolve => { this.releaseRemove = resolve })
  private markRemoveStarted!: () => void
  readonly removeStarted = new Promise<void>(resolve => { this.markRemoveStarted = resolve })

  allowRemove() { this.releaseRemove() }

  override async remove(key: string) {
    this.markRemoveStarted()
    await this.removeGate
    delete this.values[key]
  }
}

describe('offline-safe takeover settings', () => {
  it('applies a popup choice immediately and retries it after reconnect', async () => {
    const storage = new MemoryStorage()
    const desktop = vi.fn().mockRejectedValue(new Error('native host offline'))
    const sync = new TakeoverSettingsSync(storage, desktop, () => 'change-1')

    await expect(sync.queue({ enabled: false })).resolves.toMatchObject({
      ok: true,
      queued: true,
      takeover_enabled: false,
    })
    await vi.waitFor(() => expect(desktop).toHaveBeenCalledOnce())
    expect(storage.values.enabled).toBe(false)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toMatchObject({ id: 'change-1', enabled: false })

    desktop.mockResolvedValue({ ok: true, takeover_enabled: false, takeover_minimum_bytes: 0 })
    await sync.sync()
    expect(desktop).toHaveBeenLastCalledWith({ op: 'set_takeover_settings', enabled: false })
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toBeUndefined()
    expect(storage.values.enabled).toBe(false)
  })

  it('retains a protocol-rejected change without spinning until a later retry succeeds', async () => {
    const storage = new MemoryStorage()
    const desktop = vi.fn().mockResolvedValue({ ok: false, error: 'desktop rejected settings' })
    const sync = new TakeoverSettingsSync(storage, desktop, () => 'rejected-change')

    await sync.queue({ enabled: false, minimumBytes: 8192 })
    await sync.sync()

    expect(desktop).toHaveBeenCalledTimes(1)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toEqual({
      id: 'rejected-change',
      enabled: false,
      minimumBytes: 8192,
    })
    expect(storage.values.enabled).toBe(false)
    expect(storage.values.minimumBytes).toBe(8192)

    desktop.mockResolvedValue({ ok: true, takeover_enabled: false, takeover_minimum_bytes: 8192 })
    await sync.sync()

    expect(desktop).toHaveBeenCalledTimes(2)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toBeUndefined()
  })

  it('merges concurrent partial updates inside one serialized read-modify-write boundary', async () => {
    const storage = new MemoryStorage()
    const never = new Promise(() => undefined)
    const desktop = vi.fn(() => never)
    let sequence = 0
    const sync = new TakeoverSettingsSync(storage, desktop, () => `change-${++sequence}`)

    const enabled = sync.queue({ enabled: false })
    const minimum = sync.queue({ minimumBytes: 4096 })
    await Promise.all([enabled, minimum])

    expect(storage.values.enabled).toBe(false)
    expect(storage.values.minimumBytes).toBe(4096)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toEqual({
      id: 'change-2',
      enabled: false,
      minimumBytes: 4096,
    })
  })

  it('normalizes byte thresholds to the integer range accepted by the native u64 protocol', async () => {
    const storage = new MemoryStorage()
    const never = new Promise(() => undefined)
    let sequence = 0
    const sync = new TakeoverSettingsSync(storage, () => never, () => `bytes-${++sequence}`)

    await expect(sync.queue({ minimumBytes: 4096.75 })).resolves.toMatchObject({
      takeover_minimum_bytes: 4096,
    })
    expect(storage.values.minimumBytes).toBe(4096)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toMatchObject({ minimumBytes: 4096 })

    await expect(sync.queue({ minimumBytes: Number.MAX_VALUE })).resolves.toMatchObject({
      takeover_minimum_bytes: Number.MAX_SAFE_INTEGER,
    })
    expect(storage.values.minimumBytes).toBe(Number.MAX_SAFE_INTEGER)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toMatchObject({
      minimumBytes: Number.MAX_SAFE_INTEGER,
    })
  })

  it('never lets a slow older response overwrite a newer click', async () => {
    const storage = new MemoryStorage()
    let releaseFirst!: (value: unknown) => void
    const first = new Promise(resolve => { releaseFirst = resolve })
    const desktop = vi.fn()
      .mockReturnValueOnce(first)
      .mockResolvedValueOnce({ ok: true, takeover_enabled: false, takeover_minimum_bytes: 0 })
    let sequence = 0
    const sync = new TakeoverSettingsSync(storage, desktop, () => `change-${++sequence}`)

    await sync.queue({ enabled: true })
    await sync.queue({ enabled: false })
    releaseFirst({ ok: true, takeover_enabled: true, takeover_minimum_bytes: 0 })
    await sync.sync()

    expect(desktop).toHaveBeenNthCalledWith(1, { op: 'set_takeover_settings', enabled: true })
    expect(desktop).toHaveBeenNthCalledWith(2, { op: 'set_takeover_settings', enabled: false })
    expect(storage.values.enabled).toBe(false)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toBeUndefined()
  })

  it('does not let an in-flight acknowledgement delete a newer pending choice', async () => {
    const storage = new DelayedRemoveStorage()
    const desktop = vi.fn()
      .mockResolvedValueOnce({ ok: true, takeover_enabled: true, takeover_minimum_bytes: 0 })
      .mockResolvedValueOnce({ ok: true, takeover_enabled: false, takeover_minimum_bytes: 0 })
    let sequence = 0
    const sync = new TakeoverSettingsSync(storage, desktop, () => `change-${++sequence}`)

    await sync.queue({ enabled: true })
    await storage.removeStarted
    const newer = sync.queue({ enabled: false })
    storage.allowRemove()
    await newer

    await vi.waitFor(() => expect(desktop).toHaveBeenCalledTimes(2))
    await sync.sync()
    expect(desktop).toHaveBeenNthCalledWith(2, { op: 'set_takeover_settings', enabled: false })
    expect(storage.values.enabled).toBe(false)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toBeUndefined()
  })

  it('shows the pending local value instead of an older desktop ping', async () => {
    const storage = new MemoryStorage()
    const never = new Promise(() => undefined)
    const sync = new TakeoverSettingsSync(storage, () => never, () => 'queued')
    await sync.queue({ enabled: false, minimumBytes: 4096 })

    await expect(sync.applyPing({
      ok: true,
      takeover_enabled: true,
      takeover_minimum_bytes: 1024,
    })).resolves.toMatchObject({
      ok: true,
      takeover_enabled: false,
      takeover_minimum_bytes: 4096,
      takeover_settings_pending: true,
    })
  })
})