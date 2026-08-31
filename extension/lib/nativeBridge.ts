export interface NativePortLike {
  postMessage(message: Record<string, unknown>): void
  disconnect(): void
  onMessage: { addListener(listener: (message: unknown) => void): void }
  onDisconnect: { addListener(listener: () => void): void }
}

interface PendingRequest {
  message: Record<string, unknown>
  requestId: string
  timeoutMs: number
  retriesRemaining: number
  priority: number
  resolve(value: unknown): void
  reject(reason: Error): void
  timer?: ReturnType<typeof setTimeout>
}

export class NativeBridge {
  private port: NativePortLike | null = null
  private active: PendingRequest | null = null
  private readonly queue: PendingRequest[] = []
  private closed = false
  private requestSequence = 0

  constructor(
    private readonly connect: () => NativePortLike,
    private readonly timeoutMs = 30_000,
    private readonly disconnected: () => void = () => undefined,
  ) {}

  request(
    message: Record<string, unknown>,
    timeoutMs = this.timeoutMs,
    retryCount = 0,
  ): Promise<any> {
    if (this.closed) return Promise.reject(new Error('插件连接已关闭'))
    return new Promise((resolve, reject) => {
      const requestId = `${Date.now().toString(36)}-${++this.requestSequence}`
      const request: PendingRequest = {
        message: { ...message, __request_id: requestId },
        requestId,
        timeoutMs,
        retriesRemaining: Math.max(0, Math.floor(retryCount)),
        priority: this.requestPriority(message),
        resolve,
        reject,
      }
      // A user click must not wait behind a hung heartbeat/status poll. A
      // Native Messaging port cannot cancel one in-flight message, so the
      // interactive request is sent on a fresh port and the interrupted
      // request — always an idempotent read or long poll — is re-issued right
      // after it. The old port is detached before disconnect() so its final
      // event cannot reject or complete work on the replacement connection.
      if (this.active && request.priority > this.active.priority) {
        const interrupted = this.active
        if (interrupted.timer) clearTimeout(interrupted.timer)
        interrupted.timer = undefined
        this.active = null
        // pump() keeps the active request at queue[0] until its response
        // arrives; take it out before requeueing so it cannot end up queued
        // twice (or not at all) on either side of the preempting request.
        if (this.queue[0] === interrupted) this.queue.shift()
        this.queue.unshift(request)
        this.queue.splice(1, 0, interrupted)
        const port = this.port
        this.port = null
        try { port?.disconnect() } catch {}
        this.pump()
        return
      }
      // Native Messaging hosts process stdin serially. Interactive offers and
      // downloads still jump ahead of queued heartbeat/status work even when
      // the current request has equal or higher priority.
      const start = this.active ? 1 : 0
      const nextLowerPriority = this.queue.findIndex(
        (queued, index) => index >= start && queued.priority < request.priority,
      )
      if (nextLowerPriority >= 0) this.queue.splice(nextLowerPriority, 0, request)
      else this.queue.push(request)
      this.pump()
    })
  }

  private requestPriority(message: Record<string, unknown>): number {
    const operation = String(message.op || '')
    if (new Set(['offer', 'download', 'activate', 'media_push', 'set_takeover_settings']).has(operation)) return 30
    if (new Set(['handoff_status', 'media_push_status']).has(operation)) return 20
    if (operation === 'wait_handoff') return 10
    return 0
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    const error = new Error('插件连接已关闭')
    this.rejectActive(error)
    while (this.queue.length) this.queue.shift()!.reject(error)
    const port = this.port
    this.port = null
    try { port?.disconnect() } catch {}
  }

  private ensurePort(): NativePortLike {
    if (this.port) return this.port
    const port = this.connect()
    port.onMessage.addListener(message => this.handleMessage(port, message))
    port.onDisconnect.addListener(() => this.handleDisconnect(port))
    this.port = port
    return port
  }

  private pump(): void {
    if (this.closed || this.active || !this.queue.length) return
    const request = this.queue[0]
    this.active = request
    try {
      const port = this.ensurePort()
      request.timer = setTimeout(() => {
        if (this.active !== request) return
        this.retryOrRejectActive(port, new Error('插件请求超时'))
      }, request.timeoutMs)
      port.postMessage(request.message)
    } catch (error) {
      const failedPort = this.port
      this.port = null
      // connectNative() may succeed even though the first postMessage throws
      // (for example while the host is exiting). Detach that unusable port;
      // otherwise every retry leaves another native host connection alive.
      try { failedPort?.disconnect() } catch {}
      const reason = error instanceof Error ? error : new Error(String(error))
      if (request.retriesRemaining > 0 && !this.closed) {
        request.retriesRemaining -= 1
        this.active = null
        this.disconnected()
        setTimeout(() => this.pump(), 80)
      } else {
        this.rejectActive(reason)
        this.pump()
      }
    }
  }

  private handleMessage(port: NativePortLike, message: unknown): void {
    // A timed-out/disconnected port may still deliver a buffered response.
    // Never let it complete the next request running on a replacement port.
    if (this.port !== port) return
    const request = this.active
    if (!request) return
    // v7 Core echoes the request id on every response. A missing id is not a
    // response to the active request: accepting it would shift the serialized
    // queue and hand an unsolicited/malformed result to the wrong caller.
    if (!message || typeof message !== 'object') return
    const responseId = String((message as Record<string, unknown>).__request_id || '')
    if (responseId !== request.requestId) return
    if (request.timer) clearTimeout(request.timer)
    this.active = null
    this.queue.shift()
    request.resolve(message)
    this.pump()
  }

  private handleDisconnect(port: NativePortLike): void {
    if (this.port !== port) return
    this.port = null
    this.retryOrRejectActive(port, new Error('下载器连接已断开'))
  }

  private retryOrRejectActive(port: NativePortLike, error: Error): void {
    const request = this.active
    if (request?.timer) clearTimeout(request.timer)
    this.port = null
    this.disconnected()
    if (request && request.retriesRemaining > 0 && !this.closed) {
      request.retriesRemaining -= 1
      this.active = null
    } else {
      this.rejectActive(error)
    }
    try { port.disconnect() } catch {}
    this.pump()
  }

  private rejectActive(error: Error): void {
    const request = this.active
    if (!request) return
    if (request.timer) clearTimeout(request.timer)
    this.active = null
    if (this.queue[0] === request) this.queue.shift()
    request.reject(error)
  }
}
