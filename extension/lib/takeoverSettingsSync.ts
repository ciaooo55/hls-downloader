export const PENDING_TAKEOVER_SETTINGS_KEY = 'pending-takeover-settings-v1'

export interface TakeoverSettingsStorageArea {
  get(keys: string | string[]): Promise<Record<string, unknown>>
  set(items: Record<string, unknown>): Promise<void>
  remove(key: string): Promise<void>
}

export interface TakeoverSettingsUpdate {
  enabled?: boolean
  minimumBytes?: number
  quickDownloadEnabled?: boolean
  speedLimitEnabled?: boolean
}

interface PendingTakeoverSettings extends TakeoverSettingsUpdate {
  id: string
}

type DesktopRequest = (message: Record<string, unknown>) => Promise<any>

function normalizedBytes(value: unknown): number | undefined {
  if (value === undefined || value === null || value === '') return undefined
  const result = Number(value)
  if (!Number.isFinite(result)) return undefined
  return Math.min(Number.MAX_SAFE_INTEGER, Math.max(0, Math.floor(result)))
}

function readDesktopBool(response: any, ...keys: string[]): boolean | undefined {
  if (!response || typeof response !== 'object') return undefined
  for (const key of keys) {
    if (typeof response[key] === 'boolean') return response[key]
  }
  return undefined
}

function normalizePending(value: unknown): PendingTakeoverSettings | null {
  if (!value || typeof value !== 'object') return null
  const raw = value as Partial<PendingTakeoverSettings>
  const id = String(raw.id || '')
  if (!id) return null
  const pending: PendingTakeoverSettings = { id }
  if (typeof raw.enabled === 'boolean') pending.enabled = raw.enabled
  const minimumBytes = normalizedBytes(raw.minimumBytes)
  if (minimumBytes !== undefined) pending.minimumBytes = minimumBytes
  if (typeof raw.quickDownloadEnabled === 'boolean') pending.quickDownloadEnabled = raw.quickDownloadEnabled
  if (typeof raw.speedLimitEnabled === 'boolean') pending.speedLimitEnabled = raw.speedLimitEnabled
  return pending
}

/**
 * Keep the popup usable during a short Native Messaging/Core restart.
 *
 * The user's choice is written locally first and tagged with a unique id. A
 * reconnect later sends the newest pending value to the desktop. Storage
 * read-modify-write operations are serialized so concurrent partial updates
 * merge into the newest pending record instead of overwriting one another.
 */
export class TakeoverSettingsSync {
  private syncing: Promise<void> | null = null
  private storageWrite: Promise<void> = Promise.resolve()
  private syncRequested = false

  constructor(
    private readonly storage: TakeoverSettingsStorageArea,
    private readonly requestDesktop: DesktopRequest,
    private readonly createId: () => string = () => globalThis.crypto?.randomUUID?.()
      || `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`,
  ) {}

  async queue(update: TakeoverSettingsUpdate): Promise<Record<string, unknown>> {
    const response = await this.withStorageWrite(async () => {
      const stored = await this.storage.get([
        PENDING_TAKEOVER_SETTINGS_KEY,
        'enabled',
        'minimumBytes',
        'quickDownloadEnabled',
        'speedLimitEnabled',
        'speedLimitKib',
      ])
      const previous = normalizePending(stored[PENDING_TAKEOVER_SETTINGS_KEY])
      const enabled = typeof update.enabled === 'boolean'
        ? update.enabled
        : previous?.enabled
      const minimumBytes = normalizedBytes(update.minimumBytes) ?? previous?.minimumBytes
      const quickDownloadEnabled = typeof update.quickDownloadEnabled === 'boolean'
        ? update.quickDownloadEnabled
        : previous?.quickDownloadEnabled
      const speedLimitEnabled = typeof update.speedLimitEnabled === 'boolean'
        ? update.speedLimitEnabled
        : previous?.speedLimitEnabled
      const pending: PendingTakeoverSettings = {
        id: this.createId(),
        ...(typeof enabled === 'boolean' ? { enabled } : {}),
        ...(minimumBytes !== undefined ? { minimumBytes } : {}),
        ...(typeof quickDownloadEnabled === 'boolean' ? { quickDownloadEnabled } : {}),
        ...(typeof speedLimitEnabled === 'boolean' ? { speedLimitEnabled } : {}),
      }
      const local: Record<string, unknown> = { [PENDING_TAKEOVER_SETTINGS_KEY]: pending }
      if (typeof update.enabled === 'boolean') local.enabled = update.enabled
      const localMinimum = normalizedBytes(update.minimumBytes)
      if (localMinimum !== undefined) local.minimumBytes = localMinimum
      if (typeof update.quickDownloadEnabled === 'boolean') local.quickDownloadEnabled = update.quickDownloadEnabled
      if (typeof update.speedLimitEnabled === 'boolean') local.speedLimitEnabled = update.speedLimitEnabled
      await this.storage.set(local)
      const speedLimitKib = normalizedBytes(stored.speedLimitKib)
      return {
        ok: true,
        queued: true,
        takeover_enabled: typeof enabled === 'boolean' ? enabled : stored.enabled !== false,
        takeover_minimum_bytes: minimumBytes ?? normalizedBytes(stored.minimumBytes) ?? 0,
        quick_download_enabled: typeof quickDownloadEnabled === 'boolean'
          ? quickDownloadEnabled
          : stored.quickDownloadEnabled === true,
        speed_limit_enabled: typeof speedLimitEnabled === 'boolean'
          ? speedLimitEnabled
          : stored.speedLimitEnabled === true,
        ...(speedLimitKib !== undefined ? { speed_limit_kib: speedLimitKib } : {}),
      }
    })
    this.triggerSync()
    return response
  }

  async applyPing(response: any): Promise<any> {
    const pending = await this.readPending()
    if (pending) {
      this.triggerSync()
      return this.pendingResponse(response, pending)
    }
    return this.withStorageWrite(async () => {
      const newest = await this.readPending()
      if (newest) {
        this.triggerSync()
        return this.pendingResponse(response, newest)
      }
      await this.applyDesktopResponse(response)
      return response
    })
  }

  sync(): Promise<void> {
    if (this.syncing) return this.syncing
    this.syncing = this.syncLoop().finally(() => { this.syncing = null })
    return this.syncing
  }

  private triggerSync(): void {
    if (this.syncing) {
      // A newer queue/ping arrived while an older attempt was in flight. One
      // follow-up pass is enough: syncLoop already consumes any newer pending
      // record it observes before the current acknowledgement settles.
      this.syncRequested = true
      return
    }
    const active = this.sync()
    void active.finally(() => {
      if (!this.syncRequested) return
      this.syncRequested = false
      this.triggerSync()
    }).catch(() => undefined) // The durable choice remains pending until the next ping.
  }

  private async syncLoop(): Promise<void> {
    while (true) {
      const pending = await this.readPending()
      if (!pending) return
      let response: any
      try {
        response = await this.requestDesktop({
          op: 'set_takeover_settings',
          ...(typeof pending.enabled === 'boolean' ? { enabled: pending.enabled } : {}),
          ...(pending.minimumBytes !== undefined ? { minimum_bytes: pending.minimumBytes } : {}),
          ...(typeof pending.quickDownloadEnabled === 'boolean' ? { quick_download_enabled: pending.quickDownloadEnabled } : {}),
          ...(typeof pending.speedLimitEnabled === 'boolean' ? { speed_limit_enabled: pending.speedLimitEnabled } : {}),
        })
      } catch {
        // The durable pending value is retried by the next heartbeat/popup ping.
        return
      }
      // NativeBridge resolves protocol-level errors as { ok: false, error }.
      // Only an explicit acknowledgement may retire the user's durable choice;
      // rejected or malformed replies stay pending for a later reconnect/ping.
      if (response?.ok !== true) return
      const settled = await this.withStorageWrite(async () => {
        const newest = await this.readPending()
        if (!newest) return true
        if (newest.id !== pending.id) return false
        await this.storage.remove(PENDING_TAKEOVER_SETTINGS_KEY)
        await this.applyDesktopResponse(response)
        return true
      })
      if (!settled) continue
      return
    }
  }

  private async readPending(): Promise<PendingTakeoverSettings | null> {
    const stored = await this.storage.get(PENDING_TAKEOVER_SETTINGS_KEY)
    return normalizePending(stored[PENDING_TAKEOVER_SETTINGS_KEY])
  }

  private pendingResponse(response: any, pending: PendingTakeoverSettings): any {
    return {
      ...response,
      ...(typeof pending.enabled === 'boolean' ? { takeover_enabled: pending.enabled } : {}),
      ...(pending.minimumBytes !== undefined ? { takeover_minimum_bytes: pending.minimumBytes } : {}),
      ...(typeof pending.quickDownloadEnabled === 'boolean' ? { quick_download_enabled: pending.quickDownloadEnabled } : {}),
      ...(typeof pending.speedLimitEnabled === 'boolean' ? { speed_limit_enabled: pending.speedLimitEnabled } : {}),
      takeover_settings_pending: true,
    }
  }

  private async applyDesktopResponse(response: any): Promise<void> {
    const values: Record<string, unknown> = {}
    const takeoverEnabled = readDesktopBool(response, 'takeover_enabled', 'browser_takeover_enabled')
    if (takeoverEnabled !== undefined) values.enabled = takeoverEnabled
    const minimumBytes = normalizedBytes(response?.takeover_minimum_bytes)
    if (minimumBytes !== undefined) values.minimumBytes = minimumBytes
    const quickDownloadEnabled = readDesktopBool(response, 'quick_download_enabled', 'browser_quick_download_enabled')
    if (quickDownloadEnabled !== undefined) values.quickDownloadEnabled = quickDownloadEnabled
    const speedLimitEnabled = readDesktopBool(response, 'speed_limit_enabled', 'download_speed_limit_enabled')
    if (speedLimitEnabled !== undefined) values.speedLimitEnabled = speedLimitEnabled
    const speedLimitKib = normalizedBytes(response?.speed_limit_kib ?? response?.download_speed_limit_kib)
    if (speedLimitKib !== undefined) values.speedLimitKib = speedLimitKib
    const tvboxConfigured = readDesktopBool(response, 'tvbox_configured', 'tvboxConfigured')
      ?? (typeof response?.tvbox_endpoint === 'string' ? response.tvbox_endpoint.trim().length > 0 : undefined)
    if (tvboxConfigured !== undefined) values.tvboxConfigured = tvboxConfigured
    if (Object.keys(values).length) await this.storage.set(values)
  }

  private withStorageWrite<T>(operation: () => Promise<T>): Promise<T> {
    const run = this.storageWrite.then(operation, operation)
    this.storageWrite = run.then(() => undefined, () => undefined)
    return run
  }
}
