import { browser } from 'wxt/browser'
import { captureSubtitleAudio } from '../../lib/subtitleAudio'
let stop: (() => void) | undefined
let current = ''
let generation = 0
let heartbeat: ReturnType<typeof setInterval> | undefined
browser.runtime.onMessage.addListener((message, _sender, respond) => {
  if (message?.target !== 'subtitle-offscreen') return
  if (message.type === 'stop') {
    if (message.sessionId && message.sessionId !== current) return
    generation++; stop?.(); stop = undefined; current = ''
    clearInterval(heartbeat)
    respond({ ok: true }); return
  }
  if (message.type !== 'start') return
  const attempt = ++generation
  stop?.(); stop = undefined
  clearInterval(heartbeat)
  current = String(message.sessionId)
  const sessionId = current
  void (async () => {
    const stream = await navigator.mediaDevices.getUserMedia({ video: false, audio: { mandatory: {
      chromeMediaSource: 'tab', chromeMediaSourceId: message.streamId,
    } } as MediaTrackConstraints })
    const cleanup = await captureSubtitleAudio(stream, true, (audio, capturedAt) => {
      if (attempt === generation) void browser.runtime.sendMessage({ target: 'subtitle-background', type: 'audio', sessionId, audio, capturedAt })
    })
    if (attempt !== generation) { cleanup(); return }
    stop = cleanup
    stream.getAudioTracks()[0]?.addEventListener('ended', () => {
      void browser.runtime.sendMessage({ target: 'subtitle-background', type: 'capture-ended', sessionId })
    })
    // 离屏文档的心跳能在暂停视频时唤醒 MV3 worker；单靠 setInterval 不行。
    heartbeat = setInterval(() => {
      void browser.runtime.sendMessage({ target: 'subtitle-background', type: 'heartbeat', sessionId }).then(response => {
        if (!response?.ok && current === sessionId) { generation++; stop?.(); stop = undefined; clearInterval(heartbeat) }
      }).catch(() => { generation++; stop?.(); stop = undefined; clearInterval(heartbeat) })
    }, 5000)
  })().then(() => respond({ ok: attempt === generation })).catch(() => {
    if (attempt === generation) current = ''
    respond({ ok: false, error: '无法采集当前标签页音频，请从插件弹窗重新开始' })
  })
  return true
})
