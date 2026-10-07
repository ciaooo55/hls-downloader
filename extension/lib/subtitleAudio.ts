import { browser } from 'wxt/browser'
export async function waitForSubtitleAudio(start: Promise<void>): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    await Promise.race([start, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error('音频未能启动，请先播放网页视频并检查网络后重试')), 5000)
    })])
  } finally { clearTimeout(timer) }
}
export async function captureSubtitleAudio(stream: MediaStream, audible: boolean, send: (audio: string, capturedAt: number) => void): Promise<() => void> {
  const context = new AudioContext()
  try {
    if (!context.audioWorklet) throw new Error('当前页面无法处理音频，请使用 HTTPS 页面或 Chrome / Edge 标签页采集')
    await context.audioWorklet.addModule(browser.runtime.getURL('/subtitle-pcm.js'))
    const source = context.createMediaStreamSource(stream)
    const worklet = new AudioWorkletNode(context, 'hls-subtitle-pcm')
    source.connect(worklet)
    // tabCapture 会切断标签页本来的播放输出，必须重新接到扬声器。
    if (audible) source.connect(context.destination)
    const silence = context.createGain(); silence.gain.value = 0
    worklet.connect(silence); silence.connect(context.destination)
    worklet.port.onmessage = event => {
      const bytes = new Uint8Array(event.data as ArrayBuffer)
      let text = ''
      for (const byte of bytes) text += String.fromCharCode(byte)
      send(btoa(text), Date.now())
    }
    await waitForSubtitleAudio(context.resume())
    return () => {
      worklet.port.onmessage = null
      source.disconnect(); worklet.disconnect()
      stream.getTracks().forEach(track => track.stop())
      void context.close()
    }
  } catch (error) {
    stream.getTracks().forEach(track => track.stop())
    await context.close()
    throw error
  }
}
