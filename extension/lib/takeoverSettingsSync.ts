export const PENDING_TAKEOVER_SETTINGS_KEY = 'pending-takeover-settings-v1'

export interface TakeoverSettingsStorageArea {
  get(keys: string | string[]): Promise<Record<string, unknown>>
  set(items: Record<string, unknown>): Promise<void>
  remove(key: string): Promise<void>
}

export interface TakeoverSettingsUpdate {
  enabled?: boolean
  minimumBytes?: number
}

interface PendingTakeoverSettings extends TakeoverSettingsUpdate {
  id: string
  updatedAt: number
}

type DesktopRequest = (message: Record<string, unknown>) => Promise<any>

function normalizedBytes(value: unknown): number | undefined {
  if (value === undefined || value === null || value === '') return undefined
  const result = Number(value)
  return Number.isFinite(result) ? Math.max(0, result) : undefined
}

function normalizePending(value: unknown): PendingTakeoverSettings | null {
  if (!value || typeof value !== 'object') return null
  const raw = value as Partial<PendingTakeoverSettings>
  const id = String(raw.id || '')
  if (!id) return null
  const pending: PendingTakeoverSettings = {
    id,
    updatedAt: Number(raw.updatedAt) || 0,
  }
  if (typeof raw.enabled === 'boolean') pending.enabled = raw.enabled
  const minimumBytes = normalizedBytes(raw.minimumBytes)
  if (minimumBytes !== undefined) pending.minimumBytes = minimumBytes
  return pending
}

/**
 * Keep the popup usable during a short Native Messaging/Core restart.
 *
 * The user's choice is written locally first and tagged with a unique id. A
 * reconnect later sends the newest pending value to the desktop. The id is
 * checked again before clearing it, so a slow response to an older click can
 * never overwrite a newer click.
 */
export class TakeoverSettingsSync {
  private syncing: Promise<void> | null = null

  constructor(
    private readonly storage: TakeoverSettingsStorageArea,
    private readonly requestDesktop: DesktopRequest,
    private readonly createId: () => string = () => globalThis.crypto?.randomUUID?.()
      || `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`,
    private readonly now: () => number = Date.now,
  ) {}

  async queue(update: TakeoverSettingsUpdate): Promise<Record<string, unknown>> {
    const stored = await this.storage.get([PENDING_TAKEOVER_SETTINGS_KEY, 'enabled', 'minimumBytes'])
    const previous = normalizePending(stored[PENDING_TAKEOVER_SETTINGS_KEY])
    const enabled = typeof update.enabled === 'boolean'
      ? update.enabled
      : previous?.enabled
    const minimumBytes = normalizedBytes(update.minimumBytes) ?? previous?.minimumBytes
    const pending: PendingTakeoverSettings = {
      id: this.createId(),
      updatedAt: this.now(),
      ...(typeof enabled === 'boolean' ? { enabled } : {}),
      ...(minimumBytes !== undefined ? { minimumBytes } : {}),
    }
    const local: Record<string, unknown> = { [PENDING_TAKEOVER_SETTINGS_KEY]: pending }
    if (typeof update.enabled === 'boolean') local.enabled = update.enabled
    const localMinimum = normalizedBytes(update.minimumBytes)
    if (localMinimum !== undefined) local.minimumBytes = localMinimum
    await this.storage.set(local)
    void this.sync()
    return {
      ok: true,
      queued: true,
      takeover_enabled: typeof enabled === 'boolean' ? enabled : stored.enabled !== false,
      takeover_minimum_bytes: minimumBytes ?? normalizedBytes(stored.minimumBytes) ?? 0,
    }
  }

  async applyPing(response: any): Promise<any> {
    const pending = await this.readPending()
    if (!pending) {
      await this.applyDesktopResponse(response)
      return response
    }
    void this.sync()
    return {
      ...response,
      ...(typeof pending.enabled === 'boolean' ? { takeover_enabled: pending.enabled } : {}),
      ...(pending.minimumBytes !== undefined ? { takeover_minimum_bytes: pending.minimumBytes } : {}),
      takeover_settings_pending: true,
    }
  }

  sync(): Promise<void> {
    if (this.syncing) return this.syncing
    this.syncing = this.syncLoop().finally(() => { this.syncing = null })
    return this.syncing
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
        })
      } catch {
        // The durable pending value is retried by the next heartbeat/popup ping.
        return
      }
      const newest = await this.readPending()
      if (!newest) return
      if (newest.id !== pending.id) continue
      await this.storage.remove(PENDING_TAKEOVER_SETTINGS_KEY)
      await this.applyDesktopResponse(response)
      return
    }
  }

  private async readPending(): Promise<PendingTakeoverSettings | null> {
    const stored = await this.storage.get(PENDING_TAKEOVER_SETTINGS_KEY)
    return normalizePending(stored[PENDING_TAKEOVER_SETTINGS_KEY])
  }

  private async applyDesktopResponse(response: any): Promise<void> {
    const values: Record<string, unknown> = {}
    if (typeof response?.takeover_enabled === 'boolean') values.enabled = response.takeover_enabled
    const minimumBytes = normalizedBytes(response?.takeover_minimum_bytes)
    if (minimumBytes !== undefined) values.minimumBytes = minimumBytes
    if (Object.keys(values).length) await this.storage.set(values)
  }
}
