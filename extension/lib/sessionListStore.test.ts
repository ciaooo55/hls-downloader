import { describe, expect, it } from 'vitest'

import { SessionListStore } from './sessionListStore'

class DelayedStorage {
  values: Record<string, unknown> = {}
  reads = 0

  async get(key: string) {
    this.reads += 1
    await new Promise(resolve => setTimeout(resolve, 2))
    return { [key]: this.values[key] }
  }

  async set(items: Record<string, unknown>) {
    await new Promise(resolve => setTimeout(resolve, 2))
    Object.assign(this.values, items)
  }
}

describe('serialized session list storage', () => {
  it('does not lose simultaneous media captures for the same page', async () => {
    const storage = new DelayedStorage()
    const store = new SessionListStore<string>(storage)

    await Promise.all([
      store.update('page', current => [...current, 'video']),
      store.update('page', current => [...current, 'audio']),
      store.update('page', current => [...current, 'manifest']),
    ])

    expect(storage.values.page).toEqual(['video', 'audio', 'manifest'])
    expect(storage.reads).toBe(3)
  })

  it('keeps different page keys independent', async () => {
    const storage = new DelayedStorage()
    const store = new SessionListStore<string>(storage)

    await Promise.all([
      store.update('one', current => [...current, 'a']),
      store.update('two', current => [...current, 'b']),
    ])

    expect(storage.values).toMatchObject({ one: ['a'], two: ['b'] })
  })
})
