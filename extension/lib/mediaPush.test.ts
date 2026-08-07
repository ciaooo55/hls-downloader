import { describe, expect, it } from 'vitest'
import { mediaPushRequestId } from './mediaPush'

describe('mediaPushRequestId', () => {
  it('preserves the desktop request ID for later status polling', () => {
    expect(mediaPushRequestId({ ok: true, id: 'request-42' }, '投屏')).toBe('request-42')
  })

  it('rejects a response that cannot be completed or polled', () => {
    expect(() => mediaPushRequestId({ ok: true }, 'TVBox 推送')).toThrow('请求 ID')
    expect(() => mediaPushRequestId({ ok: false, error: '桌面端未就绪' }, '投屏')).toThrow('桌面端未就绪')
  })
})
