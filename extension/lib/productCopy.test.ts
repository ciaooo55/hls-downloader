import { describe, expect, it } from 'vitest'
import { classifyEngineConnectionState, engineConnectionLabel, engineStatusCopy, errorText, EXTENSION_PRODUCT_LABEL, extensionVersionLabel } from './productCopy'

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

  it('distinguishes online, user_shutdown, host_missing, and core_unreachable states cleanly', () => {
    expect(classifyEngineConnectionState({ ok: true })).toBe('online')
    expect(classifyEngineConnectionState({ ok: false, error_code: 'user_shutdown' })).toBe('user_shutdown')
    expect(classifyEngineConnectionState({ ok: false, error: '桌面端已退出 (user_shutdown)' })).toBe('user_shutdown')
    expect(classifyEngineConnectionState({ ok: false, code: 'host_missing' })).toBe('host_missing')
    expect(classifyEngineConnectionState({ ok: false, error: 'Specified native messaging host not found.' })).toBe('host_missing')
    expect(classifyEngineConnectionState({ ok: false, error: 'Core pipe closed' })).toBe('core_unreachable')

    expect(engineStatusCopy('online').label).toBe('引擎已就绪')
    expect(engineStatusCopy('user_shutdown').label).toBe('桌面端已退出 · 插件下载可自动唤醒')
    expect(engineStatusCopy('user_shutdown').helperDetail).toBe(
      '从托盘退出后，普通浏览器下载将保留在浏览器中不打扰；点击插件下载按钮或右键菜单项会自动唤醒桌面端。',
    )
    expect(engineStatusCopy('host_missing').label).toBe('未检测到桌面端 · 请先安装或运行桌面程序')
    expect(engineStatusCopy('core_unreachable').label).toBe('连接中断 · 点击重试或打开桌面端')
  })
})

describe('errorText', () => {
  it('drops the JavaScript "Error:" prefix from user-facing messages', () => {
    expect(errorText(new Error('下载器连接已断开'))).toBe('下载器连接已断开')
    expect(errorText('plain')).toBe('plain')
    expect(errorText(undefined)).toBe('')
  })
})
