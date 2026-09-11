import { describe, expect, it } from 'vitest'
import { mediaPushRequestId, mediaPushTerminalResult } from './mediaPush'

describe('mediaPushRequestId', () => {
  it('preserves the desktop request ID for later status polling', () => {
    expect(mediaPushRequestId({ ok: true, id: 'request-42' }, '投屏')).toBe('request-42')
    expect(mediaPushRequestId({ ok: true, id: '  request-43  ' }, '投屏')).toBe('request-43')
    expect(mediaPushRequestId({ ok: true, id: 'request-44', status: 'done' }, '投屏')).toBe('request-44')
  })

  it('rejects a response that cannot be completed or polled', () => {
    expect(() => mediaPushRequestId({ ok: true }, 'TVBox 推送')).toThrow('请求 ID')
    expect(() => mediaPushRequestId({ ok: true, id: 42 }, 'TVBox 推送')).toThrow('请求 ID')
    expect(() => mediaPushRequestId({ ok: true, id: '   ' }, 'TVBox 推送')).toThrow('请求 ID')
    expect(() => mediaPushRequestId({ ok: false, error: '桌面端未就绪' }, '投屏')).toThrow('桌面端未就绪')
  })

  it('rejects an immediate terminal failure returned during media-push admission', () => {
    expect(() => mediaPushRequestId({
      ok: true,
      id: 'request-overlap',
      status: 'failed',
      message: '已有投送请求正在等待设备选择',
    }, '投屏')).toThrow('已有投送请求')
    expect(() => mediaPushRequestId({
      ok: true,
      id: 'request-canceled',
      status: 'canceled',
      message: '用户取消投送',
    }, 'TVBox 推送')).toThrow('用户取消投送')
  })
})

describe('mediaPushTerminalResult', () => {
  it('returns completed desktop states and keeps pending states polling', () => {
    expect(mediaPushTerminalResult({ ok: true, status: 'done' })).toMatchObject({ status: 'done' })
    expect(mediaPushTerminalResult({ ok: true, status: 'failed', message: 'receiver failed' })).toMatchObject({ status: 'failed' })
    expect(mediaPushTerminalResult({ ok: true, status: 'pending' })).toBeNull()
    expect(mediaPushTerminalResult(null)).toBeNull()
  })

  it('turns an explicit desktop status-query error into a terminal failure', () => {
    expect(mediaPushTerminalResult({ ok: false, error: '下载器连接已断开' })).toEqual({
      status: 'failed',
      message: '下载器连接已断开',
    })
  })
})
