import { desktopTaskReadiness, handoffTerminalStatus, type BrowserHandoffPayload } from './takeover'

export const PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY = 'paused-handoff-followups-v1'

/**
 * Initial wall-clock window before an unresolved desktop confirmation is
 * rechecked by the suspension-safe follow-up carrier.
 */
export const PAUSED_HANDOFF_RESOLUTION_MS = 125_000
/** Cadence for re-checking an accepted handoff whose desktop task is still unconfirmed. */
export const PAUSED_FOLLOW_UP_RECHECK_MS = 60_000
export type PausedHandoffFollowUpPhase = 'resolution' | 'readiness'

export interface PausedHandoffFollowUp {
  downloadId: number
  handoffId: string
  phase: PausedHandoffFollowUpPhase
  /** Epoch ms when the decision window (resolution) or next re-check (readiness) is due. */
  deadline: number
  createdAt: number
}

export interface FollowUpStorageArea {
  get(key: string): Promise<Record<string, unknown>>
  set(items: Record<string, unknown>): Promise<void>
}

export function normalizePausedHandoffFollowUp(value: unknown, now: number): PausedHandoffFollowUp | null {
  if (!value || typeof value !== 'object') return null
  const item = value as Record<string, unknown>
  const handoffId = String(item.handoffId || '')
  const downloadId = Number(item.downloadId)
  if (!handoffId || !Number.isInteger(downloadId) || downloadId < 0) return null
  const deadline = Number(item.deadline)
  const createdAt = Number(item.createdAt)
  return {
    downloadId,
    handoffId,
    phase: item.phase === 'readiness' ? 'readiness' : 'resolution',
    deadline: Number.isFinite(deadline) && deadline > 0 ? deadline : now,
    createdAt: Number.isFinite(createdAt) && createdAt > 0 ? createdAt : now,
  }
}

export function normalizePausedHandoffFollowUps(value: unknown, now: number): PausedHandoffFollowUp[] {
  const seen = new Set<string>()
  return (Array.isArray(value) ? value : [])
    .map(item => normalizePausedHandoffFollowUp(item, now))
    .filter((item): item is PausedHandoffFollowUp => {
      if (!item || seen.has(item.handoffId)) return false
      seen.add(item.handoffId)
      return true
    })
    .sort((left, right) => left.deadline - right.deadline || left.createdAt - right.createdAt)
}

export type PausedFollowUpStep =
  | { kind: 'keep-paused', followUp: PausedHandoffFollowUp }
  | { kind: 'resume-download' }
  | { kind: 'remove-download' }

/**
 * Next lifecycle step for one paused browser download awaiting a desktop
 * handoff. Uncertainty never starts a second transfer: the item stays paused
 * until the desktop confirms success (remove) or failure (resume). A record
 * still waiting for the user's decision transitions to readiness handling in
 * the same step once acceptance is observed.
 */
export function stepPausedHandoffFollowUp(
  followUp: PausedHandoffFollowUp,
  handoff: BrowserHandoffPayload,
  now: number,
  recheckMs: number,
): PausedFollowUpStep {
  if (followUp.phase === 'resolution') {
    const status = String(handoff.status || '')
    if (!handoffTerminalStatus(status)) {
      // A local deadline cannot prove that the desktop rejected the handoff:
      // the Core may have accepted it while Native Messaging was reconnecting.
      // Resuming here could start a second transfer, so keep ownership parked
      // until Core reports an actual terminal decision.
      return {
        kind: 'keep-paused',
        followUp: now >= followUp.deadline
          ? { ...followUp, deadline: now + recheckMs }
          : followUp,
      }
    }
    if (status !== 'accepted') return { kind: 'resume-download' }
    followUp = { ...followUp, phase: 'readiness', deadline: now + recheckMs }
  }
  const readiness = desktopTaskReadiness(handoff)
  if (readiness === 'safe-to-remove') return { kind: 'remove-download' }
  if (readiness === 'browser-fallback') return { kind: 'resume-download' }
  return { kind: 'keep-paused', followUp: { ...followUp, deadline: now + recheckMs } }
}

/** Persistent queue of paused-download follow-ups that survives MV3 suspension. */
export class PausedHandoffFollowUpStore {
  private followUps: PausedHandoffFollowUp[] = []
  private hydrated = false
  private hydration: Promise<void> | null = null

  constructor(
    private readonly storage: FollowUpStorageArea,
    private readonly key = PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY,
    private readonly now: () => number = Date.now,
  ) {}

  hydrate(): Promise<void> {
    if (this.hydrated) return Promise.resolve()
    if (this.hydration) return this.hydration
    this.hydration = (async () => {
      try {
        const stored = await this.storage.get(this.key)
        this.followUps = normalizePausedHandoffFollowUps(stored[this.key], this.now())
      } catch {
        // In-memory follow-ups remain available when session storage is unavailable.
      } finally {
        this.hydrated = true
        this.hydration = null
      }
    })()
    return this.hydration
  }

  list(): PausedHandoffFollowUp[] {
    return [...this.followUps].sort((left, right) => left.deadline - right.deadline || left.createdAt - right.createdAt)
  }

  async remember(input: Omit<PausedHandoffFollowUp, 'createdAt'> & { createdAt?: number }): Promise<PausedHandoffFollowUp> {
    await this.hydrate()
    const record: PausedHandoffFollowUp = { ...input, createdAt: input.createdAt || this.now() }
    this.followUps = [record, ...this.followUps.filter(item => item.handoffId !== record.handoffId)]
      .sort((left, right) => right.createdAt - left.createdAt)
    await this.persist()
    return record
  }

  async drop(handoffId: string): Promise<void> {
    await this.hydrate()
    if (!this.followUps.some(item => item.handoffId === handoffId)) return
    this.followUps = this.followUps.filter(item => item.handoffId !== handoffId)
    await this.persist()
  }

  private async persist(): Promise<void> {
    try {
      await this.storage.set({ [this.key]: this.list() })
    } catch {
      // The live service worker can still act on in-memory follow-ups.
    }
  }
}
