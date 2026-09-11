import { describe, expect, it, vi } from 'vitest'
import { createRecurringAlarm, filenameDeterminationEvent, requestHeaderExtraInfo, resolveFirefoxClickIntent } from './browserCapabilities'

describe('browser capability guards', () => {
  it('does not access Chromium-only filename events in Firefox', () => {
    expect(filenameDeterminationEvent(false, {})).toBeNull()
  })

  it('registers filename determination only when Chrome exposes it', () => {
    const event = { addListener: vi.fn() }
    expect(filenameDeterminationEvent(true, { onDeterminingFilename: event })).toBe(event)
  })

  it('does not pass Chrome-only extraHeaders to Firefox', () => {
    expect(requestHeaderExtraInfo(false)).toEqual(['requestHeaders'])
    expect(requestHeaderExtraInfo(true)).toEqual(['requestHeaders', 'extraHeaders'])
  })

  it('waits for a click intent that races the Firefox request', async () => {
    const wait = vi.fn(async () => ({ href: 'https://example.test/file.zip' }))
    await expect(resolveFirefoxClickIntent(undefined, wait)).resolves.toEqual({ href: 'https://example.test/file.zip' })
    expect(wait).toHaveBeenCalledOnce()
  })

  it('keeps the requested sub-minute alarm period on Chromium', async () => {
    const create = vi.fn()
    await createRecurringAlarm({ create }, 'worker-heartbeat', 0.5, false)
    expect(create).toHaveBeenCalledOnce()
    expect(create).toHaveBeenCalledWith('worker-heartbeat', { periodInMinutes: 0.5 })
  })

  it('creates Firefox recurring alarms with the portable one-minute period', async () => {
    const create = vi.fn()
    await createRecurringAlarm({ create }, 'worker-heartbeat', 0.5, true)
    expect(create).toHaveBeenCalledOnce()
    expect(create).toHaveBeenCalledWith('worker-heartbeat', { periodInMinutes: 1 })
  })

  it('retries the portable period when Firefox alarm creation rejects', async () => {
    let attempts = 0
    const create = vi.fn(() => {
      attempts += 1
      return attempts === 1 ? Promise.reject(new Error('alarms service waking')) : Promise.resolve()
    })
    await expect(createRecurringAlarm({ create }, 'worker-heartbeat', 0.5, true)).resolves.toBeUndefined()
    expect(create).toHaveBeenCalledTimes(2)
    expect(create).toHaveBeenNthCalledWith(1, 'worker-heartbeat', { periodInMinutes: 1 })
    expect(create).toHaveBeenNthCalledWith(2, 'worker-heartbeat', { periodInMinutes: 1 })
  })

  it('falls back to the portable period when Chromium alarm creation rejects', async () => {
    const create = vi.fn(() => {
      return Promise.reject(new Error('periodInMinutes must be at least 1'))
    })
    await expect(createRecurringAlarm({ create }, 'worker-heartbeat', 0.5, false)).resolves.toBeUndefined()
    expect(create).toHaveBeenCalledTimes(2)
    expect(create).toHaveBeenLastCalledWith('worker-heartbeat', { periodInMinutes: 1 })
  })
})
