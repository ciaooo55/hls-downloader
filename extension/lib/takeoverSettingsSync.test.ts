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

describe('offline-safe takeover settings', () => {
  it('applies a popup choice immediately and retries it after reconnect', async () => {
    const storage = new MemoryStorage()
    const desktop = vi.fn().mockRejectedValue(new Error('native host offline'))
    const sync = new TakeoverSettingsSync(storage, desktop, () => 'change-1', () => 100)

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

  it('never lets a slow older response overwrite a newer click', async () => {
    const storage = new MemoryStorage()
    let releaseFirst!: (value: unknown) => void
    const first = new Promise(resolve => { releaseFirst = resolve })
    const desktop = vi.fn()
      .mockReturnValueOnce(first)
      .mockResolvedValueOnce({ ok: true, takeover_enabled: false, takeover_minimum_bytes: 0 })
    let sequence = 0
    const sync = new TakeoverSettingsSync(storage, desktop, () => `change-${++sequence}`, () => sequence)

    await sync.queue({ enabled: true })
    await sync.queue({ enabled: false })
    releaseFirst({ ok: true, takeover_enabled: true, takeover_minimum_bytes: 0 })
    await sync.sync()

    expect(desktop).toHaveBeenNthCalledWith(1, { op: 'set_takeover_settings', enabled: true })
    expect(desktop).toHaveBeenNthCalledWith(2, { op: 'set_takeover_settings', enabled: false })
    expect(storage.values.enabled).toBe(false)
    expect(storage.values[PENDING_TAKEOVER_SETTINGS_KEY]).toBeUndefined()
  })

  it('shows the pending local value instead of an older desktop ping', async () => {
    const storage = new MemoryStorage()
    const never = new Promise(() => undefined)
    const sync = new TakeoverSettingsSync(storage, () => never, () => 'queued', () => 1)
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
