import { afterEach, describe, expect, it, vi } from 'vitest'
import { ClickIntentStore } from './clickIntentStore'
import { isEarlyDirectDownloadResponse } from './directResponse'
import { sendClickAction, sendStateFromHandoff } from './downloadSendState'
import { InspectionCache } from './inspectionCache'
import { shouldInspectManifestResponse } from './manifestSniff'
import { normalizePausedHandoffFollowUps } from './pausedHandoffFollowups'
import { compactResources, isShortLivedMediaSignatureUsable, playerPlaybackResources, replayableRequestHeaders,
  shouldTakeover, suggestedResourceFilename, type MediaResource } from './resources'
import { selectedDownloadUrls } from './selectionLinks'
import { SubtitlePcm } from './subtitlePcm'
import { inheritManifestAccessQuery, removeRawQueryParameters } from './urlQuery'

const master: MediaResource = { id: 'master', url: 'https://cdn.test/master.m3u8', kind: 'hls', seenAt: 1 }
const base = 'https://cdn.test'
const runtime = vi.hoisted(() => ({ id: 'extension', onMessage: { addListener: vi.fn() }, sendMessage: vi.fn() }))
const capture = vi.hoisted(() => vi.fn())
vi.mock('wxt/browser', () => ({ browser: { runtime } }))
vi.mock('./subtitleAudio', () => ({ captureSubtitleAudio: capture }))

describe('resource handoff and playback', () => {
  it('emits usable Windows filenames after path extraction and truncation', () => {
    const name = (filename: string) => suggestedResourceFilename({ ...master, kind: 'file', filename })
    expect(name('../archive%2fLPT¹.txt')).toBe('_LPT¹.txt')
    expect(name('clip%00name.mp4')).toBe('clip_name.mp4')
    expect(name('x'.repeat(199) + '.long')).toBe('x'.repeat(199))
  })

  it('passes only valid bounded replay headers and retains later credentials', () => {
    const headers = replayableRequestHeaders({ 'Bad:Name': 'x', control: 'x\x01y', folded: 'yes\r\n', Connection: 'Keep-Alive, X-Local', 'x-local': 'private',
      ['__proto__']: 'x', constructor: 'x', prototype: 'x', oversized: 'z'.repeat(32769), authorization: ' Bearer demo ' })
    expect(Object.entries(headers)).toEqual([['authorization', 'Bearer demo']])
  })

  it('preserves unsigned query bytes and empty fields alongside a signature', () => {
    const names = new Set(['_hls_part'])
    expect(removeRawQueryParameters(base + '/m?#here', names)).toBe(base + '/m?#here')
    expect(removeRawQueryParameters(base + '/m?a=%2f&&_hls_part=1&b=x+z', names)).toBe(base + '/m?a=%2f&&b=x+z')
    expect(inheritManifestAccessQuery(base + '/m?token=z', base + '/c?a=1&&b=2')).toBe(base + '/c?a=1&&b=2&token=z')
    expect(inheritManifestAccessQuery('file:///a?token=z', 'file:///b')).toBe('file:///b')
    expect(isShortLivedMediaSignatureUsable({ ...master, url: base + '/m?S=x&E=1700000000' }, 1700000000000)).toBe(false)
  })

  it('retains new playback evidence on a full resource and a full parent playlist', () => {
    const old = Array.from({ length: 48 }, (_, n) => base + '/segment' + n)
    const next = base + '/new-segment'
    const full = { ...master, playbackUrls: old }
    const [updated] = compactResources([full, { ...master, playbackUrls: [next], seenAt: 2 }])
    expect(updated.playbackUrls).toContain(next)
    const child = { ...master, url: base + '/child.m3u8', playbackUrls: [next] }
    const [parent] = compactResources([{ ...full, variants: [{ url: child.url }] }, child])
    expect(parent.playbackUrls).toHaveLength(48)
    expect(parent.playbackUrls).toContain(next)
  })

  it('matches exact playback before applying display limits', () => {
    const other = Array.from({ length: 41 }, (_, n) => ({ ...master, url: base + '/' + n + '.m3u8' }))
    const video: MediaResource = { ...master, kind: 'media', url: base + '/movie.mp4' }
    expect(playerPlaybackResources([...other, video], { sourceUrls: [video.url], startedAt: 1 }, 1)).toEqual([video])
  })

  it('honors wildcard order, non-overlap and literal pathname components', () => {
    const matches = (pattern: string, path: string) => playerPlaybackResources([{ ...master, playbackPatterns: [base + pattern] }],
      { sourceUrls: ['blob:local'], mseResourceUrls: [base + path], startedAt: 1 }).length === 1
    expect(matches('/a.*-*ab', '/a.1-22ab')).toBe(true)
    expect(matches('/a.*-*ab', '/ax1-22ab')).toBe(false)
    expect(matches('/a*aba*ba', '/aaba')).toBe(false)
  })

  it('preserves forced download policy while rejecting unsupported targets and authentication', () => {
    const policy = { enabled: false, minimumBytes: 0, excludedHosts: [], ctrlForce: true }
    for (const url of ['data:text/plain,example', 'file:///example', 'https://accounts.google.com./',
      'https://github.com/login/oauth/authorize?client_id=demo']) expect(shouldTakeover({ ...policy, url })).toBe(false)
    expect(shouldTakeover({ ...policy, url: base + '/movie' })).toBe(true)
    expect(shouldTakeover({ enabled: true, explicitClick: true, url: 'https://cdn.site.test./file', minimumBytes: 0,
      excludedHosts: ['*.site.test'] })).toBe(false)
  })

  it('keeps HTML login pages at file URLs in the browser and excludes passive manifest hints', () => {
    const details = { type: 'main_frame', method: 'GET', statusCode: 200 }
    const response = { disposition: '', resource: { ...master, kind: 'file' as const, url: base + '/file.zip', mimeType: 'text/html' } }
    expect(isEarlyDirectDownloadResponse(details, response)).toBe(false)
    expect(isEarlyDirectDownloadResponse(details, { ...response, disposition: 'attachment' })).toBe(true)
    for (const mime of ['image/webp', 'font/woff2', 'text/html ; charset=utf-8']) expect(shouldInspectManifestResponse(base + '/playlist', mime)).toBe(false)
  })

  it('deduplicates and bounds selection output and preserves balanced punctuation', () => {
    const links = Array.from({ length: 60 }, (_, n) => '/' + n)
    const urls = selectedDownloadUrls(['file:///local', ...links, ...links], base + '/extra', base)
    expect(urls).toEqual(links.slice(0, 40).map(path => base + path))
    expect(selectedDownloadUrls([], base + '/(one)' + ')'.repeat(100), base)).toEqual([base + '/(one)'])
  })

  it('retains download ownership for unknown receipt states and discards corrupt storage rows', async () => {
    expect(sendClickAction(sendStateFromHandoff({ status: 'unrecognized' }))).toBe('recheck')
    expect(normalizePausedHandoffFollowUps([{ downloadId: null, handoffId: 'a', phase: 'cleanup' }], 100)).toEqual([])
    const store = new ClickIntentStore({ get: async () => ({ intents: [{ href: base, ctrlForce: true }] }), set: async () => {} }, 'intents', () => 10000)
    expect(await store.consume({ url: base })).toBeUndefined()
  })

  it('does not resurrect an evicted or released inspection on late failure', () => {
    const cache = new InspectionCache(1000, 1)
    cache.claim('old', 1)
    cache.claim('new', 2)
    cache.defer('old', 200, 3)
    expect(cache.claim('old', 4)).toBe(true)
    cache.release('old')
    cache.defer('old', 200, 5)
    expect(cache.claim('old', 6)).toBe(true)
  })
})

describe('subtitle capture', () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers() })

  it('preserves the valid stereo channel when the other contains non-finite PCM', () => {
    const chunks: Int16Array[] = []
    const pcm = new SubtitlePcm(16000, chunk => chunks.push(chunk))
    pcm.push([Float32Array.from([Infinity, NaN, -4]), Float32Array.from([1, .5, -4])])
    pcm.flush()
    expect([...chunks[0]]).toEqual([16384, 8192, -32768])
  })

  it('isolates offscreen control and releases a stream acquired after stop', async () => {
    await import('../entrypoints/subtitle-offscreen/main')
    const listener = runtime.onMessage.addListener.mock.calls.at(-1)![0]
    const dispatch = (type: string, sender = { id: runtime.id }, streamId = 'stream') =>
      new Promise<{ ok: boolean }>(resolve => listener({ target: 'subtitle-offscreen', type, sessionId: 'session', streamId }, sender, resolve))
    const track = { stop: vi.fn() }
    let grant!: () => void
    const mediaDevices = { getUserMedia: vi.fn(() => new Promise(resolve => { grant = () => resolve({ getTracks: () => [track] }) })) }
    vi.stubGlobal('navigator', { mediaDevices })
    const contentSender = { id: runtime.id, tab: { id: 3 } }
    expect(await dispatch('start', contentSender)).toEqual({ ok: false })
    expect(await dispatch('start', { id: runtime.id }, '')).toEqual({ ok: false })
    expect(mediaDevices.getUserMedia).not.toHaveBeenCalled()
    const started = dispatch('start')
    expect(await dispatch('stop')).toEqual({ ok: true })
    grant()
    expect(await started).toEqual({ ok: false })
    expect(track.stop).toHaveBeenCalledOnce()
    expect(capture).not.toHaveBeenCalled()
  })
})
