import { describe, expect, it } from 'vitest'
import {
  browserCleanupAction,
  canContinueTakeover,
  desktopAcceptedHandoff,
  handoffStatusLabel,
  handoffTerminalStatus,
  shouldResumeBrowserDownload,
} from './takeover'

describe('browser download takeover helpers', () => {
  it('cleans completed browser downloads by removing the file copy', () => {
    expect(browserCleanupAction('complete')).toBe('remove-file')
    expect(browserCleanupAction('in_progress')).toBe('cancel')
    expect(browserCleanupAction('interrupted')).toBe('cancel')
  })

  it('resumes only paused downloads that were not handed off', () => {
    expect(shouldResumeBrowserDownload(true, false)).toBe(true)
    expect(shouldResumeBrowserDownload(true, true)).toBe(false)
    expect(shouldResumeBrowserDownload(false, false)).toBe(false)
  })

  it('continues takeover for paused or already completed downloads', () => {
    expect(canContinueTakeover(true, 'in_progress')).toBe(true)
    expect(canContinueTakeover(false, 'complete')).toBe(true)
    expect(canContinueTakeover(false, 'in_progress')).toBe(false)
  })

  it('accepts only successful desktop handoff responses that can be presented', () => {
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'desktop', presentation_ok: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'ui-fallback', presentation_ok: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'desktop-pending', presentation_ok: true, presentation_queued: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: false, handoff: { id: 'one' } })).toBe(false)
    expect(desktopAcceptedHandoff({ ok: true })).toBe(false)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_ok: false, presentation_mode: 'none' } })).toBe(false)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation: 'failed', presentation_mode: 'desktop' } })).toBe(false)
  })

  it('maps terminal handoff statuses for popup recovery', () => {
    expect(handoffTerminalStatus('pending')).toBe(false)
    expect(handoffTerminalStatus('accepting')).toBe(false)
    expect(handoffTerminalStatus('accepted')).toBe(true)
    expect(handoffStatusLabel('accepted')).toBe('已加入')
    expect(handoffStatusLabel('canceled')).toBe('已取消')
    expect(handoffStatusLabel('expired')).toBe('已过期')
  })
})
