import { SubtitlePcm } from '../lib/subtitlePcm'
declare const sampleRate: number
declare class AudioWorkletProcessor { readonly port: MessagePort }
declare function registerProcessor(name: string, processor: typeof AudioWorkletProcessor): void
export default defineUnlistedScript(() => {
  class PcmProcessor extends AudioWorkletProcessor {
    private pcm = new SubtitlePcm(sampleRate, samples => this.port.postMessage(samples.buffer, [samples.buffer as ArrayBuffer]))
    constructor() {
      super()
      this.port.onmessage = event => {
        if (event.data?.type === 'flush') {
          this.pcm.flush()
          this.port.postMessage({ type: 'flushed' })
          return
        }
      }
    }
    process(inputs: Float32Array[][]): boolean { this.pcm.push(inputs[0] || []); return true }
  }
  registerProcessor('hls-subtitle-pcm', PcmProcessor)
})
