export interface PreparedAudio { audio: string; duration: number; position: number }
export interface PrefetchAudio { reset(epoch: number, position: number): void; stop(): void }

// 以原始 PCM 的样本计时，解码速度和网页播放倍速都不能改变语音本身。
export function subtitleWindowEnd(pcm: Uint8Array): number {
  const samples = new DataView(pcm.buffer, pcm.byteOffset, pcm.byteLength)
  let quiet = 0
  for (let offset = 0; offset < pcm.length; offset += 8000) {
    const end = Math.min(pcm.length, offset + 8000)
    let energy = 0
    for (let i = offset; i + 1 < end; i += 2) energy += (samples.getInt16(i, true) / 32768) ** 2
    const silent = Math.sqrt(energy / Math.max(1, (end - offset) / 2)) < .008
    quiet = silent ? quiet + (end - offset) / 32000 : 0
    const duration = end / 32000
    if (duration >= 2 && quiet >= .5 || duration >= 8) return end
  }
  return pcm.length
}

export function prefetchSubtitleAudio(options: {
  epoch: number; position: number; duration: number
  clock(): { position: number; running: boolean }
  prepare(position: number, duration: number, epoch: number): Promise<PreparedAudio>
  send(audio: string, position: number, epoch: number): Promise<void>
  commit(start: number, end: number, epoch: number): Promise<void>
  ready(end: number, epoch: number): boolean
  cached?(position: number): number
  failed(error: string): void
}): PrefetchAudio {
  let epoch = options.epoch
  let position = options.position
  let generation = 0
  let stopped = false
  let active = false
  let buffered = new Uint8Array()
  let bufferedAt = position
  let waiting: number | undefined
  let waitingAt = 0
  let nextSendAt = 0
  let windowStart = position
  let windowBytes = new Uint8Array()
  let offset = 0
  const encode = (bytes: Uint8Array) => {
    let value = ''
    for (const byte of bytes) value += String.fromCharCode(byte)
    return btoa(value)
  }
  const tick = async () => {
    if (stopped || active) return
    const current = generation
    const clock = options.clock()
    if (!clock.running) return
    active = true
    try {
      if (waiting != null) {
        if (!options.ready(waiting, epoch)) {
          if (Date.now() - waitingAt > 20000) throw new Error('字幕窗口翻译超时，请检查服务和网络')
          return
        }
        waiting = undefined
      }
      if (!windowBytes.length) {
        const cached = options.cached?.(position) ?? position
        if (cached > position + .001) {
          position = cached; buffered = new Uint8Array(); bufferedAt = position
        }
        if (position >= options.duration - .01 || position >= clock.position + 24) return
        if (!buffered.length) {
          const response = await options.prepare(position, Math.min(12, options.duration - position), epoch)
          if (stopped || current !== generation) return
          const raw = atob(response.audio)
          buffered = Uint8Array.from(raw, char => char.charCodeAt(0)); bufferedAt = position
          if (!buffered.length || buffered.length % 2 || buffered.length > 384000
            || Math.abs(response.position - position) > .01 || Math.abs(response.duration - buffered.length / 32000) > .01) {
            throw new Error('音轨解码返回了无效的时间范围')
          }
        }
        const end = subtitleWindowEnd(buffered)
        windowStart = bufferedAt; windowBytes = buffered.slice(0, end); offset = 0
        buffered = buffered.slice(end); bufferedAt += end / 32000
        nextSendAt = Math.max(nextSendAt, Date.now())
      }
      if (Date.now() < nextSendAt || position >= clock.position + 30) return
      const bytes = windowBytes.slice(offset, offset + 8000)
      await options.send(encode(bytes), windowStart + offset / 32000, epoch)
      if (stopped || current !== generation) return
      offset += bytes.length; position = windowStart + offset / 32000
      // 不突发追赶后台节流，也不假定云端允许超实时发送。
      nextSendAt = Date.now() + bytes.length / 32
      if (offset === windowBytes.length) {
        await options.commit(windowStart, position, epoch)
        if (stopped || current !== generation) return
        waiting = position; waitingAt = Date.now(); windowBytes = new Uint8Array()
      }
    } catch (reason) {
      if (!stopped && current === generation) {
        stopped = true; clearInterval(timer)
        options.failed(reason instanceof Error ? reason.message : '字幕预取失败')
      }
    } finally { active = false }
  }
  const timer = setInterval(() => { void tick() }, 50)
  void tick()
  return {
    reset(nextEpoch, nextPosition) {
      generation++; epoch = nextEpoch; position = nextPosition
      buffered = new Uint8Array(); bufferedAt = position; windowBytes = new Uint8Array(); offset = 0
      waiting = undefined; nextSendAt = 0
    },
    stop() { stopped = true; generation++; clearInterval(timer); buffered = new Uint8Array(); windowBytes = new Uint8Array() },
  }
}
