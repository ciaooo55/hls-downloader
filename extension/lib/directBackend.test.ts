import { afterEach, describe, expect, it, vi } from 'vitest'
import { BrowserDirectBackend } from './directBackend'

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
})
