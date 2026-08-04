export interface SessionListStorageArea {
  get(key: string): Promise<Record<string, unknown>>
  set(items: Record<string, unknown>): Promise<void>
}

/** Serialize read-modify-write operations per storage key. */
export class SessionListStore<T> {
  private readonly tails = new Map<string, Promise<void>>()

  constructor(private readonly storage: SessionListStorageArea) {}

  update(key: string, updater: (current: T[]) => T[]): Promise<T[]> {
    const previous = this.tails.get(key) || Promise.resolve()
    const operation = previous.catch(() => undefined).then(async () => {
      const stored = await this.storage.get(key)
      const current = Array.isArray(stored[key]) ? stored[key] as T[] : []
      const next = updater(current)
      await this.storage.set({ [key]: next })
      return next
    })
    const tail = operation.then(() => undefined, () => undefined)
    this.tails.set(key, tail)
    void tail.finally(() => {
      if (this.tails.get(key) === tail) this.tails.delete(key)
    })
    return operation
  }
}
