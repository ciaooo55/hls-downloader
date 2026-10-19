import { browser } from 'wxt/browser'
import { captureSubtitleAudio } from '../../lib/subtitleAudio'
let stop: (() => void | Promise<void>) | undefined
let current = ''
let generation = 0
let heartbeat: ReturnType<typeof setInterval> | undefined
browser.runtime.onMessage.addListener((message, _sender, respond) => {
  if (message?.target !== 'subtitle-offscreen') return
  if (message.type === 'stop') {
    if (message.sessionId && message.sessionId !== current) {
      respond({ ok: true })
      return
    }
    const sessionId = current
    const cleanup = stop; stop = undefined
    clearInterval(heartbeat)
    void Promise.resolve(cleanup?.()).then(() => {
      if (current === sessionId) { generation++; current = '' }
      respond({ ok: true })
    }, () => {
      if (current === sessionId) { generation++; current = '' }
      respond({ ok: false, error: '音频尾部未能提交给字幕服务' })
    })
    return true
  }
  if (message.type !== 'start') return
  const attempt = ++generation
  const previousCleanup = stop; stop = undefined
  void Promise.resolve(previousCleanup?.()).catch(() => undefined)
  clearInterval(heartbeat)
  current = String(message.sessionId)
  const sessionId = current
  void (async () => {
    const stream = await navigator.mediaDevices.getUserMedia({ video: false, audio: { mandatory: {
      chromeMediaSource: 'tab', chromeMediaSourceId: message.streamId,
    } } as MediaTrackConstraints })
    const cleanup = await captureSubtitleAudio(stream, true, (audio, capturedAt, finalFlush) => {
      if (attempt === generation) return browser.runtime.sendMessage({ target: 'subtitle-background', type: 'audio', sessionId, audio, capturedAt, finalFlush }).then(response => {
        if (!response?.ok) throw new Error('音频尾部未能提交给字幕服务')
      })
    })
    if (attempt !== generation) { await cleanup().catch(() => undefined); return }
    stop = cleanup
    stream.getAudioTracks()[0]?.addEventListener('ended', () => {
      void browser.runtime.sendMessage({ target: 'subtitle-background', type: 'capture-ended', sessionId }).catch(() => undefined)
    })
    // 离屏文档的心跳能在暂停视频时唤醒 MV3 worker；单靠 setInterval 不行。
    heartbeat = setInterval(() => {
      void browser.runtime.sendMessage({ target: 'subtitle-background', type: 'heartbeat', sessionId }).then(response => {
        if (!response?.ok && current === sessionId) {
          const cleanup = stop; stop = undefined; clearInterval(heartbeat)
          const done = () => { if (current === sessionId) { generation++; current = '' } }
          void Promise.resolve(cleanup?.()).then(done, done)
        }
      }).catch(() => {
        const cleanup = stop; stop = undefined; clearInterval(heartbeat)
        const done = () => { if (current === sessionId) { generation++; current = '' } }
        void Promise.resolve(cleanup?.()).then(done, done)
      })
    }, 5000)
  })().then(() => respond({ ok: attempt === generation })).catch(() => {
    if (attempt === generation) current = ''
    respond({ ok: false, error: '无法采集当前标签页音频，请从插件弹窗重新开始' })
  })
  return true
})
