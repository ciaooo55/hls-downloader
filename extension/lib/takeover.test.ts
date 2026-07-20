import { describe, expect, it } from 'vitest'
import { browserCleanupAction, shouldResumeBrowserDownload } from './takeover'

describe('browser takeover ownership', () => {
  it('never resumes the browser copy after desktop ownership transfers', () => {
    expect(shouldResumeBrowserDownload(true, true)).toBe(false)
    expect(shouldResumeBrowserDownload(true, false)).toBe(true)
  })

  it('removes a completed browser copy and cancels an active one', () => {
    expect(browserCleanupAction('complete')).toBe('remove-file')
    expect(browserCleanupAction('in_progress')).toBe('cancel')
    expect(browserCleanupAction('interrupted')).toBe('cancel')
  })
})
