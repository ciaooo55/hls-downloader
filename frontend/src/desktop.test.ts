import { beforeEach, describe, expect, it, vi } from 'vitest'
import { beginUninstall, getDesktopInfo } from './desktop'

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
