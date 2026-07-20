import { describe, expect, it } from 'vitest'
import { browserCleanupAction, desktopAcceptedHandoff, shouldResumeBrowserDownload } from './takeover'

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

  it('transfers ownership as soon as the desktop opens its confirmation', () => {
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one' } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: false, handoff: { id: 'one' } })).toBe(false)
    expect(desktopAcceptedHandoff({ ok: true })).toBe(false)
  })
})
