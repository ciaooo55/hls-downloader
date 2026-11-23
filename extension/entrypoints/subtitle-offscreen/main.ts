import { browser } from 'wxt/browser'
import { captureSubtitleAudio } from '../../lib/subtitleAudio'
import { withDeadline } from '../../lib/asyncDeadline'

interface CaptureSession {
  id: string
  cleanup?: () => Promise<void>
  stopping?: Promise<void>
  heartbeat?: ReturnType<typeof setTimeout>
}
let current: CaptureSession | undefined

function stop(session: CaptureSession): Promise<void> {
  if (!session.stopping) {
    clearTimeout(session.heartbeat)
    session.stopping = Promise.resolve().then(() => session.cleanup?.()).finally(() => {
      if (current === session) current = undefined
    })
  }
  return session.stopping
}

function scheduleHeartbeat(session: CaptureSession): void {
  if (current !== session || session.stopping) return
  session.heartbeat = setTimeout(async () => {
    try {
      const response = await withDeadline(browser.runtime.sendMessage({ target: 'subtitle-background', type: 'heartbeat', sessionId: session.id }), 15_000)
      if (response?.ok) { scheduleHeartbeat(session); return }
    } catch {}
    await stop(session).catch(() => undefined)
  }, 5000)
}

async function start(session: CaptureSession, streamId: string): Promise<boolean> {
  const stream = await navigator.mediaDevices.getUserMedia({ video: false, audio: { mandatory: {
    chromeMediaSource: 'tab', chromeMediaSourceId: streamId,
  } } as MediaTrackConstraints })
  if (current !== session || session.stopping) {
    stream.getTracks().forEach(track => track.stop())
    return false
  }
  const cleanup = await captureSubtitleAudio(stream, true, async (audio, capturedAt, finalFlush) => {
    if (current !== session) return
    const response = await withDeadline(browser.runtime.sendMessage({ target: 'subtitle-background', type: 'audio', sessionId: session.id, audio, capturedAt, finalFlush }), 15_000)
    if (!response?.ok) throw new Error('音频尾部未能提交给字幕服务')
  })
  if (current !== session || session.stopping) {
    await cleanup()
    return false
  }
  const track = stream.getAudioTracks()[0]
  if (!track || track.readyState === 'ended') {
    await cleanup()
    return false
  }
  const ended = () => {
    if (current !== session || session.stopping) return
    void browser.runtime.sendMessage({ target: 'subtitle-background', type: 'capture-ended', sessionId: session.id })
      .catch(() => undefined).then(() => stop(session)).catch(() => undefined)
  }
  track.addEventListener('ended', ended, { once: true })
  session.cleanup = () => {
    track.removeEventListener('ended', ended)
    return cleanup()
  }
  scheduleHeartbeat(session)
  return true
}

browser.runtime.onMessage.addListener((message, sender, respond) => {
  if (message?.target !== 'subtitle-offscreen') return
  if (sender.id !== browser.runtime.id || sender.tab || typeof message.sessionId !== 'string' || !message.sessionId) {
    respond({ ok: false })
    return
  }
  if (message.type === 'stop') {
    if (!current || current.id !== message.sessionId) { respond({ ok: true }); return }
    void stop(current).then(() => respond({ ok: true }), () => respond({ ok: false, error: '音频尾部未能提交给字幕服务' }))
    return true
  }
  if (message.type !== 'start') return
  if (typeof message.streamId !== 'string' || !message.streamId) { respond({ ok: false }); return }
  if (current) void stop(current).catch(() => undefined)
  const session: CaptureSession = { id: message.sessionId }
  current = session
  void start(session, message.streamId).then(ok => {
    if (!ok && current === session) current = undefined
    respond({ ok })
  }, () => {
    if (current === session) current = undefined
    respond({ ok: false, error: '无法采集当前标签页音频，请从插件弹窗重新开始' })
  })
  return true
})
