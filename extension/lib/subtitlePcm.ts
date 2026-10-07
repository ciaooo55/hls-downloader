// 对连续音频积分重采样，保留跨 render quantum 的相位，输出 PCM16 单声道。
export class SubtitlePcm {
  private phase = 0
  private sum = 0
  private chunk = new Int16Array(4000)
  private used = 0
  constructor(private readonly inputRate: number, private readonly emit: (pcm: Int16Array) => void) {}
  push(channels: Float32Array[]): void {
    if (!channels.length) return
    const ratio = this.inputRate / 16000
    for (let i = 0; i < channels[0].length; i++) {
      let sample = 0
      for (const channel of channels) sample += channel[i] || 0
      sample /= channels.length
      let remaining = 1
      while (remaining > 1e-9) {
        const weight = Math.min(remaining, ratio - this.phase)
        this.sum += sample * weight
        this.phase += weight
        remaining -= weight
        if (this.phase >= ratio - 1e-9) {
          const normalized = Math.max(-1, Math.min(1, this.sum / ratio))
          this.chunk[this.used++] = Math.round(normalized * (normalized < 0 ? 32768 : 32767))
          this.phase = 0; this.sum = 0
          if (this.used === this.chunk.length) {
            this.emit(this.chunk)
            this.chunk = new Int16Array(4000); this.used = 0
          }
        }
      }
    }
  }
}
