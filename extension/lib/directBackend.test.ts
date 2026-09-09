import { describe, expect, it } from 'vitest'
import {
  BrowserDirectBackend,
  shouldAttachLoopbackBridge,
  shouldClearLoopbackBridge,
  shouldRouteThroughLoopbackBridge,
} from './directBackend'

describe('v7 browser transport', () => {
  it('never attaches the retired FastAPI loopback bridge', () => {
    expect(shouldAttachLoopbackBridge({
      bridge_base: 'http://127.0.0.1:8765/api',
      bridge_token: 'secret',
    })).toBe(false)
    expect(shouldAttachLoopbackBridge({ protocol: 'hls-downloader-v7-core' })).toBe(false)
  })

  it('always clears any leftover loopback pairing state', () => {
    expect(shouldClearLoopbackBridge(null)).toBe(true)
    expect(shouldClearLoopbackBridge({ protocol: 'hls-downloader-v6-core' })).toBe(true)
    expect(shouldClearLoopbackBridge({ bridge_base: 'http://127.0.0.1:8765/api' })).toBe(true)
  })

  it('never routes v7 operations through loopback HTTP', () => {
    for (const op of ['ping', 'offer', 'download', 'handoff_status', 'activate', 'media_push']) {
      expect(shouldRouteThroughLoopbackBridge(op, true)).toBe(false)
    }
  })

  it('fails closed if the retired backend is called directly', async () => {
    const backend = new BrowserDirectBackend('http://127.0.0.1:8765/api', 'paired-secret')
    await expect(backend.request(
      { op: 'activate' },
      { version: '7.0.2', client_id: 'edge-1', browser: 'edge' },
    )).rejects.toThrow('Legacy loopback backend is disabled in v7')
  })
})
