import { describe, expect, it, vi } from 'vitest'

import { BROWSER_CLIENT_ID_STORAGE_KEY, detectBrowserFamily, stableBrowserClientId } from './browserClient'

describe('browser client identity', () => {
  it('detects the active browser family', () => {
    expect(detectBrowserFamily('moz-extension://id/background.html', 'Firefox/142')).toBe('firefox')
    expect(detectBrowserFamily('chrome-extension://id/background.html', 'Edg/150.0')).toBe('edge')
    expect(detectBrowserFamily('chrome-extension://id/background.html', 'Chrome/150.0 OPR/120.0')).toBe('opera')
    expect(detectBrowserFamily('chrome-extension://id/background.html', 'Chrome/150.0 Vivaldi/7.0')).toBe('vivaldi')
    expect(detectBrowserFamily('chrome-extension://id/background.html', 'Chrome/150.0', true)).toBe('brave')
    expect(detectBrowserFamily('chrome-extension://id/background.html', 'Chrome/150.0')).toBe('chrome')
  })

  it('creates one durable installation id', async () => {
    const data: Record<string, unknown> = {}
    const storage = {
      get: vi.fn(async () => ({ ...data })),
      set: vi.fn(async (value: Record<string, unknown>) => { Object.assign(data, value) }),
    }
    const create = vi.fn(() => 'client-one')

    await expect(stableBrowserClientId(storage, create)).resolves.toBe('client-one')
    await expect(stableBrowserClientId(storage, create)).resolves.toBe('client-one')
    expect(create).toHaveBeenCalledOnce()
    expect(data[BROWSER_CLIENT_ID_STORAGE_KEY]).toBe('client-one')
  })

  it('retries a transient storage read before creating a new installation id', async () => {
    const data: Record<string, unknown> = { [BROWSER_CLIENT_ID_STORAGE_KEY]: 'client-existing' }
    let reads = 0
    const storage = {
      get: vi.fn(async () => {
        reads += 1
        if (reads === 1) throw new Error('storage waking')
        return { ...data }
      }),
      set: vi.fn(async (value: Record<string, unknown>) => { Object.assign(data, value) }),
    }
    const create = vi.fn(() => 'client-new')

    await expect(stableBrowserClientId(storage, create)).resolves.toBe('client-existing')
    expect(storage.get).toHaveBeenCalledTimes(2)
    expect(storage.set).not.toHaveBeenCalled()
    expect(create).not.toHaveBeenCalled()
  })

  it('recovers the same id when a committed storage write loses its acknowledgement', async () => {
    const data: Record<string, unknown> = {}
    let writes = 0
    const storage = {
      get: vi.fn(async () => ({ ...data })),
      set: vi.fn(async (value: Record<string, unknown>) => {
        Object.assign(data, value)
        writes += 1
        if (writes === 1) throw new Error('lost storage acknowledgement')
      }),
    }
    const create = vi.fn(() => 'client-one')

    await expect(stableBrowserClientId(storage, create)).resolves.toBe('client-one')
    expect(storage.get).toHaveBeenCalledTimes(2)
    expect(storage.set).toHaveBeenCalledOnce()
    expect(create).toHaveBeenCalledOnce()
    expect(data[BROWSER_CLIENT_ID_STORAGE_KEY]).toBe('client-one')
  })
})
