import { describe, expect, it, vi } from 'vitest'
import { browser } from 'wxt/browser'
import { SubtitlePcm } from './subtitlePcm'
import { captureSubtitleAudio, waitForSubtitleAudio } from './subtitleAudio'
import { installSubtitleContent } from './subtitleContent'
import { resolveSubtitleAudioSource, subtitleAllowed, subtitleReadyEnd, subtitleSettings, visibleSubtitle } from './subtitleSettings'
import { parseHlsManifest } from './hlsManifest'
import { prefetchSubtitleAudio, subtitleWindowEnd } from './subtitlePrefetch'

vi.mock('wxt/browser', () => ({
  browser: {
    runtime: {
      id: 'test-ext',
      getURL: (path: string) => `moz-extension://fixture/${path}`,
      sendMessage: vi.fn(),
      onMessage: { addListener: vi.fn(), removeListener: vi.fn() },
    },
  },
}))

function audioFixture(module: Promise<void>) {
  const track = { stop: vi.fn() }
  const stream = { getTracks: () => [track] } as unknown as MediaStream
  const source = { connect: vi.fn(), disconnect: vi.fn() }
  const port = {
    onmessage: null as ((event: { data: unknown }) => void) | null,
    postMessage: () => queueMicrotask(() => port.onmessage?.({ data: { type: 'flushed' } })),
  }
  const worklet = { port, connect: vi.fn(), disconnect: vi.fn() }
  const context = { audioWorklet: { addModule: () => module }, createMediaStreamSource: () => source,
    createGain: () => ({ gain: { value: 0 }, connect: vi.fn() }),
    resume: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
  vi.stubGlobal('AudioContext', function () { return context })
  vi.stubGlobal('AudioWorkletNode', function () { return worklet })
  return { stream, track, context, source, worklet }
}

describe('提前字幕的连续窗口和取消', () => {
  it('较晚完成的字幕不能跨越未翻译的空洞，回看能复用连续就绪范围', () => {
    expect(subtitleReadyEnd([[20, 30], [0, 8]], 2)).toBe(8)
    expect(subtitleReadyEnd([[20, 30], [0, 8], [8, 20]], 2)).toBe(30)
    expect(subtitleReadyEnd([[0, 8]], 50)).toBe(50)
    // Media element clock jitter slightly behind window start (e.g. 9.95s vs [10, 18]) still chains cleanly.
    expect(subtitleReadyEnd([[10, 18], [18, 26]], 9.95)).toBe(26)
    expect(subtitleReadyEnd([[10, 18], [19, 26]], 9.95)).toBe(18)
  })

  it('resolves language-free multi-codec audio renditions while failing closed on ambiguous multi-language tracks', () => {
    const biliResource = {
      url: 'https://upos.bilivideo.com/video.m4s',
      audioUrl: 'https://upos.bilivideo.com/audio-192k.m4s',
      audioTracks: [
        { label: '音轨', language: '', url: 'https://upos.bilivideo.com/audio-192k.m4s', default: true },
        { label: '音轨', language: '', url: 'https://upos.bilivideo.com/audio-flac.m4s', default: false },
      ],
    }
    expect(resolveSubtitleAudioSource(biliResource, { audioIndex: -1, audioLanguage: '', audioLabel: '' })).toMatchObject({
      url: 'https://upos.bilivideo.com/audio-192k.m4s',
      audioUrl: undefined,
    })
    const multiLang = {
      url: 'https://cdn.example/master.m3u8',
      audioTracks: [
        { label: 'English', language: 'en', url: 'https://cdn.example/en.m3u8', default: true },
        { label: 'Japanese', language: 'ja', url: 'https://cdn.example/ja.m3u8', default: false },
      ],
    }
    expect(() => resolveSubtitleAudioSource(multiLang, { audioIndex: -1, audioLanguage: '', audioLabel: '' }))
      .toThrow('无法确认当前备用音轨')
    expect(resolveSubtitleAudioSource(multiLang, { audioIndex: 1, audioLanguage: 'ja', audioLabel: 'Japanese' })).toMatchObject({
      url: 'https://cdn.example/ja.m3u8',
      audioUrl: undefined,
    })
  })
  it('断句跨下载分片连续计时，优先在静音处提交并限制连续讲话窗口', () => {
    const pcm = new Uint8Array(12 * 32000)
    const view = new DataView(pcm.buffer)
    for (let i = 0; i < 3 * 32000; i += 2) view.setInt16(i, 4000, true)
    expect(subtitleWindowEnd(pcm)).toBe(3.5 * 32000)
    for (let i = 0; i < pcm.length; i += 2) view.setInt16(i, 4000, true)
    expect(subtitleWindowEnd(pcm)).toBe(8 * 32000)
  })

  it('发送保持正常语速，云端未完成时不推进下一个窗口，停止后不再采集', async () => {
    vi.useFakeTimers()
    let control: ReturnType<typeof prefetchSubtitleAudio> | undefined
    try {
      const prepare = vi.fn(async (position: number, duration: number) => ({ position, duration, audio: btoa('\0'.repeat(duration * 32000)) }))
      const send = vi.fn(async (_audio: string, _position: number, _epoch: number) => undefined)
      const commit = vi.fn(async () => undefined)
      let completed = false
      control = prefetchSubtitleAudio({ epoch: 0, position: 0, duration: 4,
        clock: () => ({ position: 0, running: true }), prepare, send, commit,
        ready: () => completed, failed: reason => { throw new Error(reason) } })
      await vi.advanceTimersByTimeAsync(0)
      expect(send).toHaveBeenCalledTimes(1)
      await vi.advanceTimersByTimeAsync(249)
      expect(send).toHaveBeenCalledTimes(1)
      await vi.advanceTimersByTimeAsync(2000)
      expect(commit).toHaveBeenCalledWith(0, 2, 0)
      const sent = send.mock.calls.length
      await vi.advanceTimersByTimeAsync(1000)
      expect(send.mock.calls.length).toBe(sent)
      completed = true
      await vi.advanceTimersByTimeAsync(300)
      expect(send.mock.calls.length).toBeGreaterThan(sent)
      control.stop()
      const stopped = send.mock.calls.length
      await vi.advanceTimersByTimeAsync(1000)
      expect(send.mock.calls.length).toBe(stopped)
    } finally { control?.stop(); vi.useRealTimers() }
  })

  it('通信耗时计入音频间隔，慢请求不会重叠发送或突发追赶', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(0)
    let control: ReturnType<typeof prefetchSubtitleAudio> | undefined
    try {
      const starts: number[] = []
      const positions: number[] = []
      const delays = [100, 600, 0]
      let inFlight = false
      control = prefetchSubtitleAudio({ epoch: 0, position: 0, duration: 2,
        clock: () => ({ position: 0, running: true }),
        prepare: async (position, duration) => ({ position, duration, audio: btoa('\0'.repeat(duration * 32000)) }),
        send: async (_audio, position) => {
          expect(inFlight).toBe(false)
          inFlight = true
          starts.push(Date.now()); positions.push(position)
          const delay = delays[starts.length - 1] ?? 0
          if (delay) await new Promise(resolve => setTimeout(resolve, delay))
          inFlight = false
        },
        commit: async () => undefined, ready: () => false,
        failed: reason => { throw new Error(reason) } })
      await vi.advanceTimersByTimeAsync(1600)
      expect(starts[1] - starts[0]).toBe(250)
      expect(starts[2] - starts[1]).toBeGreaterThanOrEqual(600)
      expect(starts[2] - starts[1]).toBeLessThanOrEqual(650)
      expect(starts[3] - starts[2]).toBe(250)
      expect(positions.slice(0, 4)).toEqual([0, .25, .5, .75])
    } finally { control?.stop(); vi.useRealTimers() }
  })

  it('拖动时丢弃旧解码响应，只发送新时间轴的音频', async () => {
    vi.useFakeTimers()
    let control: ReturnType<typeof prefetchSubtitleAudio> | undefined
    try {
      let release!: (value: { position: number; duration: number; audio: string }) => void
      const prepare = vi.fn().mockImplementationOnce(() => new Promise(resolve => { release = resolve }))
        .mockImplementation(async (position: number, duration: number) => ({ position, duration, audio: btoa('\0'.repeat(duration * 32000)) }))
      const send = vi.fn(async (_audio: string, _position: number, _epoch: number) => undefined)
      control = prefetchSubtitleAudio({ epoch: 0, position: 0, duration: 100,
        clock: () => ({ position: 50, running: true }), prepare, send,
        commit: async () => undefined, ready: () => false, failed: reason => { throw new Error(reason) } })
      control.reset(1, 50)
      release({ position: 0, duration: 1, audio: btoa('\0'.repeat(32000)) })
      await vi.advanceTimersByTimeAsync(100)
      expect(send).toHaveBeenCalledWith(expect.any(String), 50, 1)
      expect(send.mock.calls.every(call => call[2] === 1)).toBe(true)
    } finally { control?.stop(); vi.useRealTimers() }
  })
})

describe('字幕的实际行为边界', () => {
  it('浏览器拒绝创建音频上下文时仍须释放已取得的媒体轨道', async () => {
    const fixture = audioFixture(Promise.resolve())
    vi.stubGlobal('AudioContext', function () { throw new Error('音频设备不可用') })
    try {
      await expect(captureSubtitleAudio(fixture.stream, false, () => undefined)).rejects.toThrow('音频设备不可用')
      expect(fixture.track.stop).toHaveBeenCalledOnce()
    } finally { vi.unstubAllGlobals() }
  })

  it('音频处理模块加载不结束时必须超时并释放采集轨道', async () => {
    vi.useFakeTimers()
    const fixture = audioFixture(new Promise<void>(() => undefined))
    try {
      const capture = captureSubtitleAudio(fixture.stream, false, () => undefined)
      let failure: unknown
      void capture.catch(reason => { failure = reason })
      await vi.advanceTimersByTimeAsync(5001)
      expect(failure).toBeInstanceOf(Error)
      expect(fixture.track.stop).toHaveBeenCalledOnce()
      expect(fixture.context.close).toHaveBeenCalledOnce()
      expect(vi.getTimerCount()).toBe(0)
    } finally { vi.unstubAllGlobals(); vi.useRealTimers() }
  })

  it('同步发送错误不能逃出音频回调，停止时应报告失败并释放资源', async () => {
    const fixture = audioFixture(Promise.resolve())
    let cleanup: (() => Promise<void>) | undefined
    try {
      cleanup = await captureSubtitleAudio(fixture.stream, false, () => { throw new Error('扩展连接已失效') })
      expect(() => fixture.worklet.port.onmessage?.({ data: new Uint8Array([0, 1]).buffer })).not.toThrow()
      await expect(cleanup()).rejects.toThrow('扩展连接已失效')
      expect(fixture.source.disconnect).toHaveBeenCalledOnce()
      expect(fixture.track.stop).toHaveBeenCalledOnce()
      expect(fixture.context.close).toHaveBeenCalledOnce()
    } finally { await cleanup?.().catch(() => undefined); vi.unstubAllGlobals() }
  })

  it('自动播放权限或媒体请求不结束时启动会失败，成功和失败都不遗留定时器', async () => {
    vi.useFakeTimers()
    try {
      const pending = waitForSubtitleAudio(new Promise<void>(() => undefined))
      const rejection = expect(pending).rejects.toThrow('音频未能启动')
      await vi.runAllTimersAsync(); await rejection
      await waitForSubtitleAudio(Promise.resolve())
      expect(vi.getTimerCount()).toBe(0)
    } finally { vi.useRealTimers() }
  })
  it('全局关闭不能被站点和页面覆盖，页面规则能显式覆盖站点', () => {
    const url = 'https://video.example/watch?v=1#player'
    const settings = subtitleSettings({ enabled: false, sites: { 'video.example': true } })
    const pages = { 'https://video.example/watch?v=1': true }
    expect(subtitleAllowed(settings, pages, url)).toBe(false)
    settings.enabled = true; settings.sites['video.example'] = false
    expect(subtitleAllowed(settings, {}, url)).toBe(false)
    expect(subtitleAllowed(settings, pages, url)).toBe(true)
    pages['https://video.example/watch?v=1'] = false
    expect(subtitleAllowed(settings, pages, url)).toBe(false)
  })
  it('跨音频块的重采样不丢样本，立体声正确混为单声道 PCM16', () => {
    const chunks: Int16Array[] = []
    const pcm = new SubtitlePcm(44100, chunk => chunks.push(chunk))
    for (let offset = 0; offset < 44100; offset += 128) {
      const count = Math.min(128, 44100 - offset)
      pcm.push([new Float32Array(count).fill(1), new Float32Array(count).fill(0)])
    }
    expect(chunks).toHaveLength(4)
    expect(chunks.flatMap(chunk => [...chunk])).toHaveLength(16000)
    expect(chunks.every(chunk => chunk.every(value => Math.abs(value - 16383.5) <= 0.6))).toBe(true)
  })
  it('flush 会发送末尾不满块的实际样本，并允许后续继续累积', () => {
    const chunks: Int16Array[] = []
    const pcm = new SubtitlePcm(16000, chunk => chunks.push(chunk))
    pcm.push([new Float32Array(4500).fill(0.5)])

    expect(chunks.map(chunk => chunk.length)).toEqual([4000])
    pcm.flush()
    pcm.flush()
    expect(chunks.map(chunk => chunk.length)).toEqual([4000, 500])
    expect(chunks[1].every(value => value === 16384)).toBe(true)

    pcm.push([new Float32Array(10).fill(-0.5)])
    pcm.flush()
    expect(chunks.map(chunk => chunk.length)).toEqual([4000, 500, 10])
    expect(chunks[2].every(value => value === -16384)).toBe(true)
  })
  it('字幕微调参与快进后的缓存选择，重叠时使用最新语句', () => {
    const cues = [
      { id: 'one', start: 10, end: 13, source: '', translation: '一', final: true },
      { id: 'two', start: 12, end: 15, source: '', translation: '二', final: true },
    ]
    expect(visibleSubtitle(cues, 12.5, 0)?.id).toBe('two')
    expect(visibleSubtitle(cues, 12.5, 1)?.id).toBe('one')
    expect(visibleSubtitle(cues, 9, 0)).toBeUndefined()
    expect(visibleSubtitle(cues, 9, -2)?.id).toBe('one')
  })
  it('HLS 主清单提前发现独立音轨和内嵌音轨，保持访问签名', () => {
    const info = parseHlsManifest('#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="English",LANGUAGE="en",DEFAULT=YES,URI="audio/en.m3u8"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="中文",LANGUAGE="zh"\n#EXT-X-STREAM-INF:BANDWIDTH=100000,AUDIO="a"\nvideo.m3u8', 'https://media.example/master.m3u8?token=secret')
    expect(info.audioTracks).toHaveLength(2)
    expect(info.audioTracks[0]).toMatchObject({ language: 'en', default: true, url: 'https://media.example/audio/en.m3u8?token=secret' })
    expect(info.audioTracks[1].url).toBeUndefined()
  })

  it('无媒体元素的页面不启动 10 秒 subtitle-players 轮询，发现后轮询并在移除后发一次清空即停', async () => {
    vi.useFakeTimers()
    class FakeMediaElement {
      isConnected = true
      paused = true
      ended = false
      title = 'Demo Video'
      currentSrc = 'https://video.example/play.mp4'
      currentTime = 0
      duration = 60
      playbackRate = 1
      readyState = 4
      seeking = false
      getAttribute() { return null }
      getBoundingClientRect() { return { left: 0, top: 0, bottom: 200, width: 640, height: 360 } }
      addEventListener() {}
      removeEventListener() {}
      querySelectorAll() { return [] }
    }
    const makeNode = (tag = 'div'): any => ({
      tagName: tag.toUpperCase(),
      dataset: {},
      style: {},
      hidden: false,
      disabled: false,
      open: false,
      textContent: '',
      title: '',
      parentElement: null as any,
      setAttribute() {},
      addEventListener() {},
      removeEventListener() {},
      append(...children: any[]) { children.forEach(c => { if (c && typeof c === 'object') c.parentElement = this }) },
      remove() { this.parentElement = null },
      attachShadow() { return makeNode('shadow') },
    })
    const docEl = makeNode('html')
    const fakeDocument = {
      title: 'Non-Media Page',
      documentElement: docEl,
      fullscreenElement: null,
      createElement: (tag: string) => makeNode(tag),
      querySelectorAll: () => [] as any[],
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }
    const fakeWindow = { addEventListener: vi.fn(), removeEventListener: vi.fn() }
    vi.stubGlobal('HTMLMediaElement', FakeMediaElement)
    vi.stubGlobal('document', fakeDocument)
    vi.stubGlobal('window', fakeWindow)
    vi.stubGlobal('innerHeight', 800)

    const sendMock = browser.runtime.sendMessage as unknown as ReturnType<typeof vi.fn>
    sendMock.mockClear()
    sendMock.mockResolvedValue({ ok: true, allowed: false, settings: subtitleSettings(null), pageUrl: 'https://video.example/watch' })

    let invalidate!: () => void
    try {
      const discover = installSubtitleContent({ onInvalidated: cb => { invalidate = cb } }, () => undefined)
      await vi.advanceTimersByTimeAsync(0)
      expect(sendMock).not.toHaveBeenCalled()
      expect(vi.getTimerCount()).toBe(0)

      // Discovering the first media element reports immediately and starts the 10s poll.
      const video = new FakeMediaElement() as unknown as HTMLMediaElement
      discover(video as unknown as ParentNode)
      await vi.advanceTimersByTimeAsync(0)
      expect(sendMock).toHaveBeenCalledTimes(1)
      expect(sendMock.mock.calls[0][0]).toMatchObject({ type: 'subtitle-players', players: [expect.objectContaining({ label: 'Demo Video' })] })
      expect(vi.getTimerCount()).toBe(1)

      // Removing the media element sends a single empty cleanup update on the next tick, then stops all timers.
      ;(video as unknown as { isConnected: boolean }).isConnected = false
      await vi.advanceTimersByTimeAsync(10_000)
      expect(sendMock).toHaveBeenCalledTimes(2)
      expect(sendMock.mock.calls[1][0]).toEqual({ type: 'subtitle-players', players: [] })
      expect(vi.getTimerCount()).toBe(0)

      // Advancing further fires zero additional IPC calls.
      await vi.advanceTimersByTimeAsync(30_000)
      expect(sendMock).toHaveBeenCalledTimes(2)
    } finally {
      invalidate?.()
      vi.unstubAllGlobals()
      vi.useRealTimers()
    }
  })
})
