import { describe, expect, it, vi } from 'vitest'
import { filenameDeterminationEvent, requestHeaderExtraInfo, resolveFirefoxClickIntent } from './browserCapabilities'

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
})
