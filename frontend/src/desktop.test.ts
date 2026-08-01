import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  beginUninstall,
  getDesktopInfo,
  pickFolder,
  resizeDesktopWindow,
} from './desktop'

describe('standalone web desktop fallbacks', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    Object.defineProperty(globalThis, 'window', {
      configurable: true,
      value: { close: vi.fn() },
    })
  })

  it('reports web mode without exposing legacy native bridges', async () => {
    await expect(getDesktopInfo()).resolves.toEqual({ ok: true, installed: false, mode: 'web' })
  })

  it('does not offer uninstall outside the Tauri package', async () => {
    await expect(beginUninstall()).resolves.toEqual({ ok: false, error: '卸载仅在安装版中可用' })
  })

  it('reports that native folder picking is unavailable', async () => {
    await expect(pickFolder()).resolves.toEqual({ ok: false, error: 'native-folder-unavailable' })
  })

  it('does not resize a standalone browser window', async () => {
    await expect(resizeDesktopWindow(390, 320)).resolves.toEqual({
      ok: false,
      error: 'native-resize-unavailable',
    })
  })
})
