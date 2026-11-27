// Native Messaging is the extension transport to the resident Core.

const MAX_NATIVE_RESOURCE_TITLE_CODE_UNITS = 4096
/**
 * Keep the Native Messaging port alive across Chrome's 1-minute MV3 worker
 * alarm while still rotating before Core's 120-second idle IPC close.
 */
export const PORT_IDLE_MS = 95_000
/**
 * Brief grace window before tearing down an in-flight `ping` / `get_settings`
 * port for a higher-priority request so fast ~2ms replies complete cleanly.
 */
export const PREEMPT_GRACE_MS = 250

function scalarSafeNativeText(value: string): string {
  let result = ''
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1)
      if (next >= 0xdc00 && next <= 0xdfff) {
        result += value[index] + value[index + 1]
        index += 1
      } else result += '\uFFFD'
      continue
    }
    result += code >= 0xdc00 && code <= 0xdfff ? '\uFFFD' : value[index]
  }
  return result
}

function boundedNativeResourceTitle(value: string): string {
  let truncated = value.slice(0, MAX_NATIVE_RESOURCE_TITLE_CODE_UNITS)
  if (/[\uD800-\uDBFF]$/.test(truncated)) truncated = truncated.slice(0, -1)
  return scalarSafeNativeText(truncated)
}

function snapshotNativeValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(snapshotNativeValue)
  if (!value || typeof value !== 'object') return value
  const snapshot: Record<string, unknown> = {}
  for (const [key, nested] of Object.entries(value as Record<string, unknown>)) {
    snapshot[key] = snapshotNativeValue(nested)
  }
  return snapshot
}

function nativeMessageWithSafeResourceStrings(message: Record<string, unknown>): Record<string, unknown> {
  const resource = message.resource
  if (!resource || typeof resource !== 'object' || Array.isArray(resource)) return message
  const record = resource as Record<string, unknown>
  // Snapshot resource metadata when the request enters the queue. Resource
  // payloads contain nested request headers/contexts, so a shallow copy would
  // still let caller mutations alter a queued Native Messaging request later.
  const normalized = snapshotNativeValue(record) as Record<string, unknown>
  if (typeof record.title === 'string') {
    normalized.title = boundedNativeResourceTitle(record.title)
  }
  if (typeof record.filename === 'string') {
    normalized.filename = scalarSafeNativeText(record.filename)
  }
  return { ...message, resource: normalized }
}

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
  startedAt?: number
  posted?: boolean
  resolve(value: unknown): void
  reject(reason: Error): void
  timer?: ReturnType<typeof setTimeout>
  fallbackResponse?: unknown
}

// Chrome: "Specified native messaging host not found." / "Native host has exited."
// Firefox: "No such native application …". Only the "not installed" family is
// mapped to the explicit desktop-missing message.
const NATIVE_HOST_MISSING = /native messaging host not found|no such native application/i
const NATIVE_HOST_FORBIDDEN = /access to the specified native messaging host is forbidden/i

export type NativeErrorCode = 'host_missing' | 'host_forbidden' | 'host_disconnected'
export type NativeBridgeErrorCode = 'queue_full' | 'bridge_closed'

/** Transport failure with a stable code; callers must not parse the Chinese message. */
export class NativeHostError extends Error {
  constructor(message: string, readonly code: NativeErrorCode) {
    super(message)
    this.name = 'NativeHostError'
  }
}

/**
 * Local NativeBridge admission/lifecycle failure.
 * Only errors with `definiteUnsent === true` prove that `port.postMessage` was
 * never invoked for this request.
 */
export class NativeBridgeError extends Error {
  constructor(
    message: string,
    readonly code: NativeBridgeErrorCode,
    readonly definiteUnsent: boolean,
  ) {
    super(message)
    this.name = 'NativeBridgeError'
  }
}

export function isDefiniteUnsentNativeError(error: unknown): error is NativeBridgeError {
  if (error instanceof NativeBridgeError) return error.definiteUnsent === true
  return Boolean(
    error
    && typeof error === 'object'
    && (error as { name?: unknown }).name === 'NativeBridgeError'
    && (error as { definiteUnsent?: unknown }).definiteUnsent === true,
  )
}

export function nativeDisconnectError(detail: string): NativeHostError {
  return NATIVE_HOST_MISSING.test(detail)
    ? new NativeHostError('下载器连接已断开：未检测到 HLS Downloader 桌面端，请确认主程序已安装并运行', 'host_missing')
    : NATIVE_HOST_FORBIDDEN.test(detail)
      ? new NativeHostError('下载器连接已断开：浏览器禁止访问下载器，请检查扩展授权与浏览器集成注册', 'host_forbidden')
      : new NativeHostError('下载器连接已断开', 'host_disconnected')
}

/** Reason the browser gives for a native port disconnect (Firefox port.error, Chrome runtime.lastError). */
export function nativeDisconnectDetail(port: unknown): string {
  const portError = (port as { error?: { message?: unknown } } | null)?.error?.message
  if (portError) return String(portError)
  try {
    const runtime = (globalThis as { chrome?: { runtime?: { lastError?: { message?: unknown } } } }).chrome?.runtime
    return String(runtime?.lastError?.message || '')
  } catch {
    return ''
  }
}

function usesGracePreemption(message: Record<string, unknown>): boolean {
  const op = String(message.op || '')
  if (op === 'ping') return message.interactive !== true
  return op === 'get_settings' || op === 'get_takeover_settings'
}

export class NativeBridge {
  private port: NativePortLike | null = null
  private portLastResponseAt = 0
  private active: PendingRequest | null = null
  private preemptTimer: ReturnType<typeof setTimeout> | null = null
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
    if (this.closed) {
      return Promise.reject(new NativeBridgeError('插件连接已关闭', 'bridge_closed', true))
    }
    if (this.queue.length >= 64) {
      return Promise.reject(new NativeBridgeError('插件请求过多，请等待当前操作完成后重试', 'queue_full', true))
    }
    return new Promise((resolve, reject) => {
      const requestId = `${Date.now().toString(36)}-${++this.requestSequence}`
      const request: PendingRequest = {
        message: { ...nativeMessageWithSafeResourceStrings(message), __request_id: requestId },
        requestId,
        timeoutMs,
        retriesRemaining: Math.max(0, Math.floor(retryCount)),
        priority: this.requestPriority(message),
        resolve,
        reject,
      }
      // A user click must not wait behind a hung heartbeat/status poll. For fast
      // read probes (`ping` / `get_settings`), allow a 250ms grace window first
      // so healthy ~2ms responses complete on the existing port without killing
      // and respawning NativeHost.exe.
      if (this.active && request.priority > this.active.priority) {
        if (usesGracePreemption(this.active.message)) {
          const elapsed = this.active.startedAt !== undefined ? Date.now() - this.active.startedAt : 0
          if (elapsed < PREEMPT_GRACE_MS) {
            const start = 1
            const nextLowerPriority = this.queue.findIndex(
              (queued, index) => index >= start && queued.priority < request.priority,
            )
            if (nextLowerPriority >= 0) this.queue.splice(nextLowerPriority, 0, request)
            else this.queue.push(request)
            this.scheduleGracePreemption(PREEMPT_GRACE_MS - Math.max(0, elapsed))
            return
          }
        }
        const start = 1
        const nextLowerPriority = this.queue.findIndex(
          (queued, index) => index >= start && queued.priority < request.priority,
        )
        if (nextLowerPriority >= 0) this.queue.splice(nextLowerPriority, 0, request)
        else this.queue.push(request)
        this.preemptActive()
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

  private scheduleGracePreemption(delayMs: number): void {
    if (this.preemptTimer) return
    this.preemptTimer = setTimeout(() => {
      this.preemptTimer = null
      if (this.closed || !this.active) return
      const nextQueued = this.queue[1]
      if (nextQueued && nextQueued.priority > this.active.priority) {
        this.preemptActive()
      }
    }, Math.max(0, delayMs))
  }

  private clearPreemptTimer(): void {
    if (!this.preemptTimer) return
    clearTimeout(this.preemptTimer)
    this.preemptTimer = null
  }

  private preemptActive(): void {
    this.clearPreemptTimer()
    const interrupted = this.active
    if (!interrupted) return
    if (interrupted.timer) clearTimeout(interrupted.timer)
    interrupted.timer = undefined
    interrupted.startedAt = undefined
    this.active = null
    if (this.queue[0] === interrupted) this.queue.shift()
    const insertAt = this.queue.findIndex(queued => queued.priority <= interrupted.priority)
    if (insertAt >= 0) this.queue.splice(insertAt, 0, interrupted)
    else this.queue.push(interrupted)
    const port = this.port
    this.port = null
    try { port?.disconnect() } catch {}
    this.pump()
  }

  private requestPriority(message: Record<string, unknown>): number {
    // 只有已知只读请求能被抢占；本地文件交接等写操作不能因遗漏分类而断线重放。
    // 字幕使用独立连接，所有动作保持同优先级，PCM 不抢断正在进行的 poll。
    switch (String(message.op || '')) {
      // 交接前的探测不能排在挂起的后台长轮询后；普通心跳仍保持最低优先级。
      case 'ping': return message.interactive === true ? 30 : 0
      case 'get_settings':
      case 'get_takeover_settings': return 10
      case 'handoff_status':
      case 'media_push_status': return 20
      default: return 30
    }
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    this.clearPreemptTimer()
    if (this.active) {
      this.rejectActive(new NativeBridgeError('插件连接已关闭', 'bridge_closed', this.active.posted !== true))
    }
    while (this.queue.length) {
      const queued = this.queue.shift()!
      queued.reject(new NativeBridgeError('插件连接已关闭', 'bridge_closed', queued.posted !== true))
    }
    const port = this.port
    this.port = null
    try { port?.disconnect() } catch {}
  }

  private ensurePort(): NativePortLike {
    // Core 会在空闲 120 秒后关闭 IPC。保持 95 秒以跨越 Chrome 1 分钟 MV3 心跳，避免每分钟杀进程。
    if (this.port && Date.now() - this.portLastResponseAt >= PORT_IDLE_MS) {
      const expired = this.port
      this.port = null
      try { expired.disconnect() } catch {}
    }
    if (this.port) return this.port
    const port = this.connect()
    port.onMessage.addListener(message => this.handleMessage(port, message))
    port.onDisconnect.addListener(() => this.handleDisconnect(port, nativeDisconnectDetail(port)))
    this.port = port
    this.portLastResponseAt = Date.now()
    return port
  }

  private pump(): void {
    if (this.closed || this.active || !this.queue.length) return
    const request = this.queue[0]
    this.active = request
    request.startedAt = Date.now()
    try {
      const port = this.ensurePort()
      request.timer = setTimeout(() => {
        if (this.active !== request) return
        this.retryOrRejectActive(port, new Error('插件请求超时'))
      }, request.timeoutMs)
      request.posted = true
      port.postMessage(request.message)
    } catch (error) {
      this.clearPreemptTimer()
      if (request.timer) clearTimeout(request.timer)
      request.timer = undefined
      request.startedAt = undefined
      const failedPort = this.port
      this.port = null
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
    if (!message || typeof message !== 'object') return
    const response = message as Record<string, unknown>
    const responseId = String(response.__request_id || '')
    if (responseId !== request.requestId) return
    this.clearPreemptTimer()
    this.portLastResponseAt = Date.now()
    const isUserShutdown = response.user_shutdown === true
      || String(response.error_code ?? '').trim().toLowerCase() === 'user_shutdown'
      || String(response.code ?? '').trim().toLowerCase() === 'user_shutdown'
    const coreTransportFailed = response.ok === false && (
      response.transport_error === true
      || (!isUserShutdown && response.code === 'core_unreachable')
      || /^Core (?:pipe|IPC|stream|TCP) /i.test(String(response.error ?? ''))
    )
    if (coreTransportFailed && request.retriesRemaining > 0) {
      this.retryOrRejectActive(port, new Error(String(response.error || '下载引擎连接中断')))
      return
    }
    if (request.timer) clearTimeout(request.timer)
    request.timer = undefined
    if (request.fallbackResponse !== undefined && (coreTransportFailed || response.ok === false)) {
      if (coreTransportFailed) {
        this.port = null
        try { port.disconnect() } catch {}
        this.disconnected()
      }
      this.resolveActiveFallback()
      this.pump()
      return
    }
    const resolvedResponse = String(request.message.op || '') === 'media_push_status'
      && response.ok === false
      && !['done', 'failed', 'canceled'].includes(String(response.status || ''))
      ? {
          ...response,
          status: 'failed',
          message: String(response.error || '读取桌面端推送状态失败'),
        }
      : coreTransportFailed ? { ...response, transport_error: true } : response
    if (this.tryMediaPushAdmissionCheck(port, request, resolvedResponse)) return
    this.active = null
    this.queue.shift()
    if (coreTransportFailed) {
      this.port = null
      try { port.disconnect() } catch {}
      this.disconnected()
    }
    request.resolve(resolvedResponse)
    this.pump()
  }

  private tryMediaPushAdmissionCheck(
    port: NativePortLike,
    request: PendingRequest,
    response: Record<string, unknown>,
  ): boolean {
    if (String(request.message.op || '') !== 'media_push'
      || response.ok !== true
      || String(response.status || '') !== 'pending') return false
    const id = typeof response.id === 'string' ? response.id.trim() : ''
    if (!id) return false

    request.fallbackResponse = response
    request.message = {
      op: 'media_push_status',
      request_id: id,
      __request_id: request.requestId,
    }
    try {
      request.timer = setTimeout(() => {
        if (this.active !== request) return
        this.retryOrRejectActive(port, new Error('投送状态确认超时'))
      }, Math.min(request.timeoutMs, 3_000))
      port.postMessage(request.message)
    } catch {
      if (request.timer) clearTimeout(request.timer)
      request.timer = undefined
      const failedPort = this.port
      this.port = null
      this.disconnected()
      this.resolveActiveFallback()
      try { failedPort?.disconnect() } catch {}
      this.pump()
    }
    return true
  }

  private handleDisconnect(port: NativePortLike, detail = ''): void {
    if (this.port !== port) return
    this.port = null
    this.retryOrRejectActive(port, nativeDisconnectError(detail))
  }

  private retryOrRejectActive(port: NativePortLike, error: Error): void {
    this.clearPreemptTimer()
    const request = this.active
    if (request?.timer) clearTimeout(request.timer)
    this.port = null
    this.disconnected()
    if (this.resolveActiveFallback()) {
      try { port.disconnect() } catch {}
      this.pump()
      return
    }
    if (request && request.retriesRemaining > 0 && !this.closed) {
      request.retriesRemaining -= 1
      request.startedAt = undefined
      this.active = null
    } else {
      this.rejectActive(error)
    }
    try { port.disconnect() } catch {}
    this.pump()
  }

  private resolveActiveFallback(): boolean {
    const request = this.active
    if (!request || request.fallbackResponse === undefined) return false
    if (request.timer) clearTimeout(request.timer)
    request.timer = undefined
    this.active = null
    if (this.queue[0] === request) this.queue.shift()
    request.resolve(request.fallbackResponse)
    return true
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
