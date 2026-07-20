export interface NativePortLike {
  postMessage(message: Record<string, unknown>): void
  disconnect(): void
  onMessage: { addListener(listener: (message: unknown) => void): void }
  onDisconnect: { addListener(listener: () => void): void }
}

interface PendingRequest {
  message: Record<string, unknown>
  timeoutMs: number
  resolve(value: unknown): void
  reject(reason: Error): void
  timer?: ReturnType<typeof setTimeout>
}

export class NativeBridge {
  private port: NativePortLike | null = null
  private active: PendingRequest | null = null
  private readonly queue: PendingRequest[] = []
  private closed = false

  constructor(
    private readonly connect: () => NativePortLike,
    private readonly timeoutMs = 30_000,
    private readonly disconnected: () => void = () => undefined,
  ) {}

  request(message: Record<string, unknown>, timeoutMs = this.timeoutMs): Promise<any> {
    if (this.closed) return Promise.reject(new Error('Native Messaging connection is closed'))
    return new Promise((resolve, reject) => {
      this.queue.push({ message, timeoutMs, resolve, reject })
      this.pump()
    })
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    const error = new Error('Native Messaging connection is closed')
    this.rejectActive(error)
    while (this.queue.length) this.queue.shift()!.reject(error)
    const port = this.port
    this.port = null
    try { port?.disconnect() } catch {}
  }

  private ensurePort(): NativePortLike {
    if (this.port) return this.port
    const port = this.connect()
    port.onMessage.addListener(message => this.handleMessage(message))
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
        if (this.port === port) this.port = null
        this.rejectActive(new Error('Native Messaging response timed out'))
        this.disconnected()
        try { port.disconnect() } catch {}
        this.pump()
      }, request.timeoutMs)
      port.postMessage(request.message)
    } catch (error) {
      this.port = null
      this.rejectActive(error instanceof Error ? error : new Error(String(error)))
      this.pump()
    }
  }

  private handleMessage(message: unknown): void {
    const request = this.active
    if (!request) return
    if (request.timer) clearTimeout(request.timer)
    this.active = null
    this.queue.shift()
    request.resolve(message)
    this.pump()
  }

  private handleDisconnect(port: NativePortLike): void {
    if (this.port !== port) return
    this.port = null
    this.rejectActive(new Error('Native Messaging host disconnected'))
    this.disconnected()
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
