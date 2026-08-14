import { describe, expect, it } from 'vitest'
import {
  browserCleanupAction,
  canContinueTakeover,
  canResumeBrowserDownload,
  desktopTaskReadiness,
  desktopAcceptedHandoff,
  mayDiscardBrowserTransfer,
  handoffStatusLabel,
  handoffTerminalStatus,
} from './takeover'

describe('browser download takeover helpers', () => {
  it('cleans completed browser downloads by removing the file copy', () => {
    expect(browserCleanupAction('complete')).toBe('remove-file')
    expect(browserCleanupAction('in_progress')).toBe('cancel')
    expect(browserCleanupAction('interrupted')).toBe('cancel')
  })

  it('observes live browser downloads and paused transient interruptions', () => {
    expect(canContinueTakeover('in_progress')).toBe(true)
    expect(canContinueTakeover('complete')).toBe(true)
    expect(canContinueTakeover('interrupted')).toBe(false)
    expect(canContinueTakeover('interrupted', true)).toBe(true)
  })

  it('resumes a paused item even when Chromium transiently marks it interrupted', () => {
    expect(canResumeBrowserDownload('in_progress')).toBe(true)
    expect(canResumeBrowserDownload('interrupted')).toBe(true)
    expect(canResumeBrowserDownload('complete')).toBe(false)
  })

  it('accepts only successful desktop handoff responses that can be presented', () => {
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'desktop', presentation_ok: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'native-shell', presentation_ok: true, presentable: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'native-shell-pending', presentation_ok: true, presentation_queued: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'ui-fallback', presentation_ok: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_mode: 'desktop-pending', presentation_ok: true, presentation_queued: true } })).toBe(true)
    expect(desktopAcceptedHandoff({ ok: false, handoff: { id: 'one' } })).toBe(false)
    expect(desktopAcceptedHandoff({ ok: true })).toBe(false)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation_ok: false, presentation_mode: 'none' } })).toBe(false)
    expect(desktopAcceptedHandoff({ ok: true, handoff: { id: 'one', presentation: 'failed', presentation_mode: 'desktop' } })).toBe(false)
  })

  it('does not discard the browser transfer just because a confirmation window opened', () => {
    expect(desktopAcceptedHandoff({
      ok: true,
      handoff: { id: 'one', status: 'pending', presentation_ok: true, presentation_mode: 'desktop' },
    })).toBe(true)
    expect(mayDiscardBrowserTransfer('pending', 'waiting')).toBe(false)
    expect(mayDiscardBrowserTransfer('accepted', 'waiting')).toBe(false)
    expect(mayDiscardBrowserTransfer('accepted', 'safe-to-remove')).toBe(true)
    expect(mayDiscardBrowserTransfer('rejected', 'browser-fallback')).toBe(false)
  })

  it('maps terminal handoff statuses for popup recovery', () => {
    expect(handoffTerminalStatus('pending')).toBe(false)
    expect(handoffTerminalStatus('accepting')).toBe(false)
    expect(handoffTerminalStatus('accepted')).toBe(true)
    expect(handoffStatusLabel('accepted')).toBe('已加入')
    expect(handoffStatusLabel('canceled')).toBe('已取消')
    expect(handoffStatusLabel('expired')).toBe('已过期')
    // connection_lost is a local transient observation; the desktop may
    // reconnect and report the real accepted/rejected state afterwards.
    expect(handoffTerminalStatus('connection_lost')).toBe(false)
    expect(handoffStatusLabel('connection_lost')).toBe('连接中断')
  })

  it('keeps Chromium fallback until the desktop transfer proves progress', () => {
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'downloading', task_stage: 'probing' }))
      .toBe('waiting')
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'downloading', task_stage: 'downloading', task_downloaded_bytes: 1 }))
      .toBe('safe-to-remove')
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'done', task_downloaded_bytes: 0 }))
      .toBe('safe-to-remove')
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'failed', task_stage: 'probing', task_error_code: 'HTTP_404' }))
      .toBe('browser-fallback')
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'paused', task_downloaded_bytes: 0 }))
      .toBe('waiting')
    expect(desktopTaskReadiness({ status: 'rejected' })).toBe('browser-fallback')
  })
})
