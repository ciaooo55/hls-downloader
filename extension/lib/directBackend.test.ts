import { afterEach, describe, expect, it, vi } from 'vitest'
import { BrowserDirectBackend, shouldAttachLoopbackBridge, shouldClearLoopbackBridge, shouldRouteThroughLoopbackBridge } from './directBackend'

afterEach(() => vi.unstubAllGlobals())

describe('BrowserDirectBackend', () => {
  it('sends offers directly with the paired token', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ id: 'handoff-1' }), { status: 200, headers: { 'Content-Type': 'application/json' } }))
    vi.stubGlobal('fetch', fetchMock)
    const backend = new BrowserDirectBackend('http://127.0.0.1:8765/api', 'paired-secret')
    const response = await backend.request({ op: 'offer', resource: { url: 'https://cdn.test/a.mp4' } }, { version: '3.0.7', client_id: 'edge-1', browser: 'edge' })
    expect(response.handoff.id).toBe('handoff-1')
    expect(fetchMock.mock.calls[0][0]).toBe('http://127.0.0.1:8765/api/browser/handoffs')
    expect((fetchMock.mock.calls[0][1].headers as Record<string, string>)['X-Token']).toBe('paired-secret')
  })

  it('routes leftover push_to_tv through the device-picker media-push path', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ ok: true, id: 'push-1' }), { status: 200, headers: { 'Content-Type': 'application/json' } }))
    vi.stubGlobal('fetch', fetchMock)
    const backend = new BrowserDirectBackend('http://127.0.0.1:8765/api', 'paired-secret')
    const response = await backend.request({ op: 'push_to_tv', resource: { url: 'https://cdn.test/a.m3u8' } }, { version: '5.0.14', client_id: 'edge-1', browser: 'edge' })
    expect(response.id).toBe('push-1')
    expect(fetchMock.mock.calls[0][0]).toBe('http://127.0.0.1:8765/api/browser/media-push')
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({
      kind: 'tvbox',
      resource: { url: 'https://cdn.test/a.m3u8' },
    })
  })

  it('does not attach FastAPI loopback when the host speaks the v7 Core protocol', () => {
    expect(
      shouldAttachLoopbackBridge({
        protocol: 'hls-downloader-v7-core',
        bridge_base: 'http://127.0.0.1:8765/api',
        bridge_token: 'secret',
      }),
    ).toBe(false)
  })

  it('clears a stale FastAPI pairing once the host speaks v7 Core', () => {
    expect(
      shouldClearLoopbackBridge({
        protocol: 'hls-downloader-v7-core',
      }),
    ).toBe(true)
    expect(shouldClearLoopbackBridge({ protocol: 'hls-downloader-v6-core' })).toBe(true)
    expect(
      shouldClearLoopbackBridge({
        protocol: 'hls-downloader-core',
        bridge_base: 'http://127.0.0.1:8765/api',
      }),
    ).toBe(false)
  })

  it('still pairs the frozen 5.x FastAPI bridge when protocol is absent', () => {
    expect(
      shouldAttachLoopbackBridge({
        bridge_base: 'http://127.0.0.1:8765/api',
        bridge_token: 'secret',
      }),
    ).toBe(true)
  })

  it('keeps heartbeat ping on Native Messaging so a v7 host can drop a stale FastAPI pairing', () => {
    expect(shouldRouteThroughLoopbackBridge('offer', true)).toBe(false)
    expect(shouldRouteThroughLoopbackBridge('download', true)).toBe(false)
    expect(shouldRouteThroughLoopbackBridge('handoff_status', true)).toBe(false)
    expect(shouldRouteThroughLoopbackBridge('ping', true)).toBe(false)
    expect(shouldRouteThroughLoopbackBridge('activate', true)).toBe(true)
    expect(shouldRouteThroughLoopbackBridge('download', false)).toBe(false)
  })
})
