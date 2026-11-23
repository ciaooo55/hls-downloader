import { expect, it, vi } from 'vitest'
import { downloadSelectionKey, selectMediaVariant, type MediaResource } from './resources'

const { browser, native } = vi.hoisted(() => {
  const event = () => ({ addListener: vi.fn() })
  const storage = () => {
    const values: Record<string, unknown> = {}
    return { get: async () => ({ ...values }), set: async (data: object) => Object.assign(values, data),
      remove: async (key: string) => { delete values[key] } }
  }
  return {
    native: vi.fn(),
    browser: {
      runtime: { getManifest: () => ({ version: '10' }), getURL: () => 'chrome-extension://test/',
        onInstalled: event(), onMessage: event() },
      webRequest: { onSendHeaders: event(), onBeforeRequest: event(), onBeforeRedirect: event(),
        onHeadersReceived: event(), onCompleted: event(), onErrorOccurred: event() },
      downloads: { onCreated: event(), onChanged: event() },
      tabs: { onActivated: event(), onUpdated: event(), onRemoved: event(),
        get: async () => ({ url: 'https://page.test/' }), sendMessage: async () => {} },
      action: { setBadgeText: async () => {} },
      contextMenus: { onClicked: event(), removeAll: async () => {}, create: vi.fn() },
      alarms: { onAlarm: event(), create: async () => {}, clear: async () => true },
      storage: { local: storage(), session: storage() },
    },
  }
})
vi.mock('wxt/browser', () => ({ browser }))
vi.mock('./nativeBridge', () => ({ NativeBridge: class { request = native } }))
vi.mock('./subtitleBackground', () => ({ installSubtitleBackground: vi.fn() }))
vi.mock('./hlsInspection', () => ({ inspectHlsResource: async () => null }))

it('offers a selected rendition through the refreshed signed master with its original quality preferences', async () => {
  vi.stubGlobal('defineBackground', (main: () => void) => ({ main }))
  try {
    native.mockImplementation(async message => message.op === 'offer'
      ? { ok: true, handoff: { id: 'chosen', status: 'pending' } }
      : { ok: true, handoff: { id: 'chosen', status: 'accepted' } })
    const background = await import('../entrypoints/background')
    ;(background.default as unknown as { main(): void }).main()
    const pageUrl = 'https://page.test/'
    const master: MediaResource = { id: 'master', kind: 'hls', seenAt: Date.now(), tabId: 7, frameId: 0,
      pageUrl, url: 'https://cdn.test/master.m3u8?s=old&e=1000000000' }
    const fresh = 'https://cdn.test/master.m3u8?s=fresh&e=4102444800'
    const request = { url: fresh, requestId: 'master-request', method: 'GET', tabId: 7, frameId: 0,
      type: 'xmlhttprequest', documentUrl: pageUrl }
    browser.webRequest.onSendHeaders.addListener.mock.calls[0][0]({ ...request,
      requestHeaders: [{ name: 'X-Playback-Token', value: 'fresh-header' }] })
    browser.webRequest.onHeadersReceived.addListener.mock.calls[0][0]({ ...request, statusCode: 200 })
    const quality = { bandwidth: 2000000, height: 720 }
    const selected = selectMediaVariant(master, { ...quality, url: 'https://cdn.test/b.m3u8' })
    const other = selectMediaVariant(master, { ...quality, url: 'https://cdn.test/a.m3u8' })
    expect(downloadSelectionKey(selected)).not.toBe(downloadSelectionKey(other))
    const result = await new Promise(resolve => browser.runtime.onMessage.addListener.mock.calls[0][0](
      { type: 'download-now', resource: selected }, { tab: { id: 7 }, frameId: 0, url: pageUrl }, resolve))
    expect(result).toMatchObject({ ok: true })
    expect(native.mock.calls.find(([message]) => message.op === 'offer')?.[0].resource).toMatchObject({
      url: fresh, preferred_bandwidth: quality.bandwidth, preferred_height: quality.height,
      request_headers: { 'x-playback-token': 'fresh-header' },
    })
  } finally {
    vi.unstubAllGlobals()
  }
})
