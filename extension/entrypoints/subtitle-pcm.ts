import { SubtitlePcm } from '../lib/subtitlePcm'
declare const sampleRate: number
declare class AudioWorkletProcessor { readonly port: MessagePort }
declare function registerProcessor(name: string, processor: typeof AudioWorkletProcessor): void
export default defineUnlistedScript(() => {
  class PcmProcessor extends AudioWorkletProcessor {
    private epoch: number | undefined
    private enabled = true
    private position = 0
    private rate = 1
    private limit = Infinity
    private pcm = this.createPcm()
    private createPcm() {
      return new SubtitlePcm(sampleRate, samples => {
        const position = this.position
        this.position += samples.length / 16000 * this.rate
        this.port.postMessage(this.epoch == null ? samples.buffer
          : { pcm: samples.buffer, epoch: this.epoch, position, rate: this.rate }, [samples.buffer as ArrayBuffer])
        if (this.epoch != null && this.position >= this.limit) this.enabled = false
      })
    }
    constructor() {
      super()
      this.port.onmessage = event => {
        if (event.data?.type === 'window') { this.limit = event.data.limit; return }
        if (event.data?.type !== 'timeline') return
        this.epoch = event.data.epoch; this.enabled = event.data.enabled === true
        this.position = event.data.position; this.rate = event.data.rate; this.limit = event.data.limit
        this.pcm = this.createPcm()
      }
    }
    process(inputs: Float32Array[][]): boolean { if (this.enabled) this.pcm.push(inputs[0] || []); return true }
  }
  registerProcessor('hls-subtitle-pcm', PcmProcessor)
})
