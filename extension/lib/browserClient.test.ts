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
})
