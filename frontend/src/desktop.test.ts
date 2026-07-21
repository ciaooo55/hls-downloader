import { beforeEach, describe, expect, it, vi } from 'vitest'
import { beginUninstall, getDesktopInfo, pickFolder } from './desktop'

describe('desktop uninstall bridge', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    Object.defineProperty(globalThis, 'window', {
      configurable: true,
      value: {
        pywebview: {
          api: {
            get_desktop_info: vi.fn().mockResolvedValue({ ok: true, installed: true, mode: 'installed' }),
            begin_uninstall: vi.fn().mockResolvedValue({ ok: true }),
            choose_folder: vi.fn().mockResolvedValue({ ok: true, path: 'D:\\Downloads' }),
          },
        },
      },
    })
  })

  it('reports whether the desktop package is installed', async () => {
    await expect(getDesktopInfo()).resolves.toEqual({ ok: true, installed: true, mode: 'installed' })
  })

  it('starts the native uninstaller', async () => {
    await expect(beginUninstall()).resolves.toEqual({ ok: true })
    expect(window.pywebview?.api.begin_uninstall).toHaveBeenCalledOnce()
  })
})


describe('desktop folder picker', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    Object.defineProperty(globalThis, 'window', {
      configurable: true,
      value: {
        pywebview: {
          api: {
            choose_folder: vi.fn().mockResolvedValue({ ok: true, path: 'D:\\Downloads' }),
          },
        },
      },
    })
  })

  it('uses the native folder dialog when available', async () => {
    await expect(pickFolder('C:\\data')).resolves.toEqual({ ok: true, path: 'D:\\Downloads' })
    expect(window.pywebview?.api.choose_folder).toHaveBeenCalledWith('C:\\data')
  })

  it('reports when native folder picking is unavailable', async () => {
    Object.defineProperty(globalThis, 'window', {
      configurable: true,
      value: { pywebview: { api: {} } },
    })
    await expect(pickFolder()).resolves.toEqual({ ok: false, error: 'native-folder-unavailable' })
  })
})
