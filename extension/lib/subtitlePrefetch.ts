import { browser } from 'wxt/browser'
import { waitForSubtitleAudio } from './subtitleAudio'

export interface PrefetchAudio { reset(epoch: number): void; stop(): void }

// 独立解码同一资源，原播放器暂停时仍能准备后续字幕；原播放器的时间和速率不受影响。
export async function prefetchSubtitleAudio(
  media: HTMLMediaElement, signal: AbortSignal, initialEpoch: number,
  running: () => boolean,
  send: (audio: string, position: number, rate: number, epoch: number) => void,
  failed: (reason: string) => void,
): Promise<PrefetchAudio> {
  if (!/^https?:/i.test(media.currentSrc) || !Number.isFinite(media.duration) || media.duration <= 0 || media.mediaKeys) {
    throw new Error('当前播放器未提供可独立读取的点播音轨')
  }
  const tracks = (media as any).audioTracks as ArrayLike<{ enabled: boolean }> | undefined
  if (tracks && Array.from(tracks).some((track, index) => index > 0 && track.enabled)) {
    throw new Error('当前选择了备用音轨，使用播放器采集以保持音轨一致')
  }
  const audio = new Audio()
  // 强制 CORS：跨域或重定向无授权时明确失败，不能把 Web Audio 的静音当成有效音轨。
  audio.crossOrigin = media.crossOrigin || 'anonymous'
  audio.preload = 'auto'
  const context = new AudioContext()
  let worklet: AudioWorkletNode | undefined
  let source: MediaElementAudioSourceNode | undefined
  let timer: ReturnType<typeof setInterval> | undefined
  let epoch = initialEpoch
  let enabled = false
  let stopped = false
  let playing: Promise<void> | undefined
  let preparedPosition = media.currentTime
  const gate = (value: boolean) => {
    if (value === enabled) return
    enabled = value
    worklet?.port.postMessage({ type: 'timeline', epoch, enabled, position: audio.currentTime,
      rate: audio.playbackRate, limit: media.currentTime + 30 })
  }
  const stop = () => {
    if (stopped) return
    stopped = true; clearInterval(timer); signal.removeEventListener('abort', stop)
    audio.pause(); audio.removeAttribute('src'); audio.load()
    if (worklet) { worklet.port.onmessage = null; worklet.disconnect() }
    source?.disconnect(); void context.close()
  }
  signal.addEventListener('abort', stop, { once: true })
  try {
    if (signal.aborted) throw new Error('字幕预取已取消')
    if (!context.audioWorklet) throw new Error('当前页面不支持音轨预取，请使用 HTTPS 页面')
    await context.audioWorklet.addModule(browser.runtime.getURL('/subtitle-pcm.js'))
    await new Promise<void>((resolve, reject) => {
      const cleanup = () => {
        clearTimeout(timeout); audio.removeEventListener('loadedmetadata', loaded)
        audio.removeEventListener('error', failed); signal.removeEventListener('abort', aborted)
      }
      const loaded = () => { cleanup(); resolve() }
      const failed = () => { cleanup(); reject(new Error('音轨无法独立读取，可能不支持格式或跨域访问')) }
      const aborted = () => { cleanup(); reject(new Error('字幕预取已取消')) }
      const timeout = setTimeout(() => { cleanup(); reject(new Error('独立音轨加载超时')) }, 8000)
      audio.addEventListener('loadedmetadata', loaded); audio.addEventListener('error', failed)
      signal.addEventListener('abort', aborted, { once: true })
      if (signal.aborted) { aborted(); return }
      audio.src = media.currentSrc
    })
    if (stopped || signal.aborted) throw new Error('字幕预取已取消')
    if (!Number.isFinite(audio.duration)) throw new Error('直播音轨使用实时采集')
    source = context.createMediaElementSource(audio)
    worklet = new AudioWorkletNode(context, 'hls-subtitle-pcm')
    source.connect(worklet)
    // worklet 只分析输入，输出为静音，预取音轨绝不送到扬声器。
    worklet.connect(context.destination)
    const reset = (nextEpoch: number) => {
      epoch = nextEpoch; gate(false); audio.pause()
      audio.currentTime = Math.min(media.currentTime, audio.duration)
      audio.playbackRate = media.playbackRate
      preparedPosition = audio.currentTime
      worklet!.port.postMessage({ type: 'timeline', epoch, enabled: false, position: audio.currentTime,
        rate: audio.playbackRate, limit: media.currentTime + 30 })
    }
    reset(epoch)
    worklet.port.onmessage = event => {
      if (stopped || !enabled || !running() || audio.paused || audio.seeking || audio.readyState < 3 || event.data.epoch !== epoch) return
      // 从实际 PCM 样本计时，避免后台页面的媒体时钟与音频时钟更新不同步。
      const position = Number(event.data.position)
      if (!Number.isFinite(position) || position > media.currentTime + 30) { gate(false); audio.pause(); return }
      preparedPosition = position + .25 * audio.playbackRate
      const bytes = new Uint8Array(event.data.pcm as ArrayBuffer)
      let text = ''
      for (const byte of bytes) text += String.fromCharCode(byte)
      send(btoa(text), position, audio.playbackRate, epoch)
    }
    await waitForSubtitleAudio(context.resume())
    if (context.state !== 'running') throw new Error('浏览器阻止了音轨预取，请先在网页播放视频')
    // 首次 play 必须确认成功，失败交回实时采集，不能留下静默的“预翻译中”。
    await waitForSubtitleAudio(audio.play()); audio.pause()
    audio.addEventListener('waiting', () => gate(false))
    audio.addEventListener('error', () => { if (!stopped) failed('独立音轨读取失败，请重新开始翻译') })
    const tick = () => {
      if (stopped) return
      const ahead = Math.max(audio.currentTime, preparedPosition) - media.currentTime
      worklet!.port.postMessage({ type: 'window', limit: media.currentTime + 30 })
      const ready = running() && !audio.ended && !audio.seeking
      // 30 秒窗口带滞回，避免边界上频繁暂停和恢复。
      if (!ready || ahead >= 30) { gate(false); audio.pause(); return }
      if (ahead > 29.5 && !enabled) return
      if (audio.paused && !playing) {
        playing = audio.play().catch(() => {
          if (!stopped) { gate(false); failed('浏览器阻止音轨预取，请重新播放网页并重试') }
        }).finally(() => { playing = undefined })
      }
      gate(!playing && !audio.paused && audio.readyState >= 3)
    }
    timer = setInterval(tick, 100)
    tick()
    return { reset, stop }
  } catch (reason) { stop(); throw reason }
}
