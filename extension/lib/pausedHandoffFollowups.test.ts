import { describe, expect, it } from 'vitest'

import {
  PAUSED_FOLLOW_UP_RECHECK_MS,
  PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY,
  PAUSED_HANDOFF_RESOLUTION_MS,
  PausedHandoffFollowUpStore,
  normalizePausedHandoffFollowUps,
  stepPausedHandoffFollowUp,
  type FollowUpStorageArea,
  type PausedHandoffFollowUp,
} from './pausedHandoffFollowups'

class MemoryStorage implements FollowUpStorageArea {
  values: Record<string, unknown> = {}
  sets = 0

  async get(key: string): Promise<Record<string, unknown>> {
    return { [key]: this.values[key] }
  }

  async set(items: Record<string, unknown>): Promise<void> {
    this.sets += 1
    Object.assign(this.values, items)
  }
}

class DelayedFirstSetStorage extends MemoryStorage {
  private first = true
  private releaseFirst!: () => void
  private readonly firstGate = new Promise<void>(resolve => { this.releaseFirst = resolve })
  private markFirstStarted!: () => void
  readonly firstStarted = new Promise<void>(resolve => { this.markFirstStarted = resolve })

  allowFirst() { this.releaseFirst() }

  override async set(items: Record<string, unknown>): Promise<void> {
    this.sets += 1
    if (this.first) {
      this.first = false
      this.markFirstStarted()
      await this.firstGate
    }
    Object.assign(this.values, items)
  }
}

class FailingFirstGetStorage extends MemoryStorage {
  private failNextGet = true

  override async get(key: string): Promise<Record<string, unknown>> {
    if (this.failNextGet) {
      this.failNextGet = false
      throw new Error('storage temporarily unavailable')
    }
    return super.get(key)
  }
}

function followUp(overrides: Partial<PausedHandoffFollowUp> = {}): PausedHandoffFollowUp {
  return {
    downloadId: 7, handoffId: 'handoff-1', phase: 'resolution',
    deadline: 130_000, createdAt: 10_000, ...overrides,
  }
}

describe('paused handoff follow-up store', () => {
  it('rehydrates persisted follow-ups and skips malformed rows after a restart', async () => {
    const storage = new MemoryStorage()
    storage.values[PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY] = [
      followUp(),
      { downloadId: 9, handoffId: '' },
      { downloadId: 8, handoffId: 'handoff-2', phase: 'readiness', deadline: 5 },
      followUp({ handoffId: 'handoff-1', downloadId: 99 }),
    ]
    const store = new PausedHandoffFollowUpStore(storage, PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY, () => 4_000)
    await store.hydrate()
    expect(store.list()).toEqual([
      followUp({ handoffId: 'handoff-2', downloadId: 8, phase: 'readiness', deadline: 5, createdAt: 4_000 }),
      followUp({ deadline: 129_000, createdAt: 4_000 }),
    ])
  })

  it('does not overwrite unread durable follow-ups after a transient storage failure', async () => {
    const storage = new FailingFirstGetStorage()
    storage.values[PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY] = [
      followUp({ handoffId: 'persisted', downloadId: 1, deadline: 100_000 }),
    ]
    const store = new PausedHandoffFollowUpStore(storage, PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY, () => 20_000)

    await store.remember({
      downloadId: 2,
      handoffId: 'live',
      phase: 'resolution',
      deadline: 110_000,
    })

    expect(storage.sets).toBe(0)
    expect(storage.values[PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY]).toEqual([
      followUp({ handoffId: 'persisted', downloadId: 1, deadline: 100_000 }),
    ])
    expect(store.list().map(item => item.handoffId)).toEqual(['live'])

    await store.hydrate()
    expect(store.list().map(item => item.handoffId)).toEqual(['persisted', 'live'])
    expect((storage.values[PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY] as PausedHandoffFollowUp[])
      .map(item => item.handoffId)).toEqual(['persisted', 'live'])
  })

  it('upserts one record per handoff without discarding unresolved browser ownership', async () => {
    const storage = new MemoryStorage()
    const store = new PausedHandoffFollowUpStore(storage, PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY, () => 1_000)
    await store.remember({ downloadId: 1, handoffId: 'handoff-1', phase: 'resolution', deadline: 61_000 })
    await store.remember({ downloadId: 1, handoffId: 'handoff-1', phase: 'readiness', deadline: 62_000 })
    expect(store.list()).toEqual([
      { downloadId: 1, handoffId: 'handoff-1', phase: 'readiness', deadline: 62_000, createdAt: 1_000 },
    ])
    for (let index = 0; index < 29; index += 1) {
      await store.remember({ downloadId: index, handoffId: `handoff-${index}`, phase: 'resolution', deadline: 63_000 })
    }
    expect(store.list().length).toBe(29)
    expect(storage.values[PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY]).toHaveLength(29)
  })

  it('serializes whole-list persistence so an older snapshot cannot erase a newer handoff', async () => {
    const storage = new DelayedFirstSetStorage()
    let now = 1_000
    const store = new PausedHandoffFollowUpStore(storage, PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY, () => now++)

    const first = store.remember({ downloadId: 1, handoffId: 'handoff-1', phase: 'resolution', deadline: 61_000 })
    await storage.firstStarted
    const second = store.remember({ downloadId: 2, handoffId: 'handoff-2', phase: 'resolution', deadline: 62_000 })

    storage.allowFirst()
    await Promise.all([first, second])

    expect(store.list().map(item => item.handoffId)).toEqual(['handoff-1', 'handoff-2'])
    expect((storage.values[PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY] as PausedHandoffFollowUp[])
      .map(item => item.handoffId)).toEqual(['handoff-1', 'handoff-2'])
  })

  it('drops a finished follow-up without writing again for unknown ids', async () => {
    const storage = new MemoryStorage()
    const store = new PausedHandoffFollowUpStore(storage, PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY, () => 1_000)
    await store.drop('handoff-missing')
    expect(storage.sets).toBe(0)
    await store.remember({ downloadId: 3, handoffId: 'handoff-1', phase: 'resolution', deadline: 61_000 })
    await store.drop('handoff-1')
    expect(store.list()).toEqual([])
    expect(storage.values[PAUSED_HANDOFF_FOLLOWUPS_STORAGE_KEY]).toEqual([])
  })
})

describe('paused handoff follow-up steps', () => {
  it('keeps waiting while the desktop confirmation is pending and fresh', () => {
    const step = stepPausedHandoffFollowUp(followUp(), { status: 'pending' }, 129_999, 60_000)
    expect(step).toEqual({ kind: 'keep-paused', followUp: followUp() })
  })

  it('keeps the paused download when the local confirmation window elapses', () => {
    for (const handoff of [{ status: 'pending' }, {}]) {
      expect(stepPausedHandoffFollowUp(followUp(), handoff, 130_000, 60_000))
        .toEqual({ kind: 'keep-paused', followUp: followUp({ deadline: 190_000 }) })
    }
  })

  it('resumes the paused download when the user rejects or the handoff fails', () => {
    for (const status of ['rejected', 'canceled', 'expired', 'failed']) {
      expect(stepPausedHandoffFollowUp(followUp(), { status }, 20_000, 60_000))
        .toEqual({ kind: 'resume-download' })
    }
  })

  it('removes the paused download as soon as the accepted task proves progress', () => {
    const accepted = (handoff: Record<string, unknown>) =>
      stepPausedHandoffFollowUp(followUp(), { status: 'accepted', ...handoff }, 20_000, 60_000)
    expect(accepted({ task_status: 'done' })).toEqual({ kind: 'remove-download' })
    expect(accepted({ task_downloaded_bytes: 128 })).toEqual({ kind: 'remove-download' })
    expect(accepted({ task_stage: 'merging' })).toEqual({ kind: 'remove-download' })
  })

  it('resumes the paused download when the accepted task definitively failed', () => {
    expect(stepPausedHandoffFollowUp(followUp(), { status: 'accepted', task_status: 'failed' }, 20_000, 60_000))
      .toEqual({ kind: 'resume-download' })
  })

  it('keeps an accepted handoff parked on the readiness re-check cadence', () => {
    const step = stepPausedHandoffFollowUp(followUp(), { status: 'accepted' }, 20_000, 60_000)
    expect(step).toEqual({
      kind: 'keep-paused',
      followUp: followUp({ phase: 'readiness', deadline: 80_000 }),
    })
  })

  it('keeps a readiness record parked when the desktop is unreachable', () => {
    const parked = followUp({ phase: 'readiness', deadline: 40_000, createdAt: 9_000 })
    const step = stepPausedHandoffFollowUp(parked, {}, 50_000, 60_000)
    expect(step).toEqual({ kind: 'keep-paused', followUp: { ...parked, deadline: 110_000 } })
  })
})

describe('paused handoff follow-up normalization', () => {
  it('sorts by deadline and drops duplicate handoff ids', () => {
    const normalized = normalizePausedHandoffFollowUps([
      followUp({ handoffId: 'handoff-c', deadline: 200_000 }),
      followUp({ handoffId: 'handoff-a', deadline: 100_000 }),
      followUp({ handoffId: 'handoff-b', deadline: 150_000 }),
      followUp({ handoffId: 'handoff-a', deadline: 90_000 }),
    ], 1_000)
    expect(normalized.map(item => item.deadline)).toEqual([100_000, 126_000, 126_000])
  })

  it('bounds restored future timestamps after the system clock moves backward', () => {
    const now = 10_000
    const normalized = normalizePausedHandoffFollowUps([
      followUp({ handoffId: 'resolution', deadline: 10_000_000, createdAt: 9_000_000 }),
      followUp({ handoffId: 'readiness', phase: 'readiness', deadline: 10_000_000, createdAt: 8_000_000 }),
    ], now)
    const byId = Object.fromEntries(normalized.map(item => [item.handoffId, item]))

    expect(byId.resolution.deadline).toBe(now + PAUSED_HANDOFF_RESOLUTION_MS)
    expect(byId.readiness.deadline).toBe(now + PAUSED_FOLLOW_UP_RECHECK_MS)
    expect(byId.resolution.createdAt).toBe(now)
    expect(byId.readiness.createdAt).toBe(now)
  })
})
