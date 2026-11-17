import { describe, expect, it } from 'vitest'
import { engineConnectionLabel, errorText, EXTENSION_PRODUCT_LABEL, extensionVersionLabel } from './productCopy'

describe('extension product language', () => {
  it('uses user-facing product names and an explicit version', () => {
    expect(EXTENSION_PRODUCT_LABEL).toBe('浏览器插件')
    expect(extensionVersionLabel('7.0.0')).toBe('版本 7.0.0')
  })

  it('describes the engine connection without exposing process architecture', () => {
    expect(engineConnectionLabel(true, false)).toBe('下载引擎已连接')
    expect(engineConnectionLabel(false, true)).toBe('下载引擎正在重连')
    expect(engineConnectionLabel(false, false)).toBe('下载引擎未连接')
  })
})

describe('errorText', () => {
  it('drops the JavaScript "Error:" prefix from user-facing messages', () => {
    expect(errorText(new Error('下载器连接已断开'))).toBe('下载器连接已断开')
    expect(errorText('plain')).toBe('plain')
    expect(errorText(undefined)).toBe('')
  })
})
