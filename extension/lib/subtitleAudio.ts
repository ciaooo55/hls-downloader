import { browser } from 'wxt/browser'
export async function waitForSubtitleAudio(start: Promise<void>): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    await Promise.race([start, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error('音频未能启动，请先播放网页视频并检查网络后重试')), 5000)
    })])
  } finally { clearTimeout(timer) }
}
export async function captureSubtitleAudio(stream: MediaStream, audible: boolean, send: (audio: string, capturedAt: number, finalFlush: boolean) => void | Promise<unknown>): Promise<() => Promise<void>> {
  let context: AudioContext
  try { context = new AudioContext() }
  catch (error) {
    stream.getTracks().forEach(track => track.stop())
    throw error
  }
  try {
    if (!context.audioWorklet) throw new Error('当前页面无法处理音频，请使用 HTTPS 页面或 Chrome / Edge 标签页采集')
    await waitForSubtitleAudio(context.audioWorklet.addModule(browser.runtime.getURL('/subtitle-pcm.js')))
    const source = context.createMediaStreamSource(stream)
    const worklet = new AudioWorkletNode(context, 'hls-subtitle-pcm')
    source.connect(worklet)
    // tabCapture 会切断标签页本来的播放输出，必须重新接到扬声器。
    if (audible) source.connect(context.destination)
    const silence = context.createGain(); silence.gain.value = 0
    worklet.connect(silence); silence.connect(context.destination)
    const pendingSends = new Set<Promise<unknown>>()
    let flushComplete: (() => void) | undefined
    let sendError: unknown
    worklet.port.onmessage = event => {
      if (event.data?.type === 'flushed') { flushComplete?.(); flushComplete = undefined; return }
      const bytes = new Uint8Array(event.data as ArrayBuffer)
      let text = ''
      for (const byte of bytes) text += String.fromCharCode(byte)
      const capturedAt = Date.now()
      const finalFlush = flushComplete !== undefined
      const pending = Promise.resolve().then(() => send(btoa(text), capturedAt, finalFlush)).catch(reason => { sendError = reason })
      pendingSends.add(pending)
      void pending.finally(() => pendingSends.delete(pending))
    }
    await waitForSubtitleAudio(context.resume())
    let cleanup: Promise<void> | undefined
    return () => {
      if (cleanup) return cleanup
      cleanup = (async () => {
        let flushError: unknown
        try {
          await new Promise<void>((resolve, reject) => {
            const timer = setTimeout(() => {
              flushComplete = undefined
              reject(new Error('音频尾部处理超时'))
            }, 2000)
            flushComplete = () => { clearTimeout(timer); resolve() }
            worklet.port.postMessage({ type: 'flush' })
          })
        } catch (reason) { flushError = reason }
        try {
          await Promise.all([...pendingSends])
          if (sendError || flushError) throw sendError || flushError
        } finally {
          worklet.port.onmessage = null
          source.disconnect(); worklet.disconnect()
          stream.getTracks().forEach(track => track.stop())
          await context.close()
        }
      })()
      return cleanup
    }
  } catch (error) {
    stream.getTracks().forEach(track => track.stop())
    await context.close()
    throw error
  }
}
