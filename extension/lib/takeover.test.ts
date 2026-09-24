import { describe, expect, it, vi } from 'vitest'
import { findEarlyBrowserTakeoverByUrl, type EarlyBrowserTakeover } from '../entrypoints/background'

vi.hoisted(() => {
  ;(globalThis as Record<string, unknown>).defineBackground = (main: unknown) => ({ main })
})

vi.mock('wxt/browser', () => ({
  browser: {
    contextMenus: {},
    storage: { session: { get: vi.fn().mockResolvedValue({}), set: vi.fn().mockResolvedValue(undefined) } },
  },
}))
import {
  browserCleanupAction,
  canContinueTakeover,
  canResumeBrowserDownload,
  desktopTaskReadiness,
  desktopAcceptedHandoff,
  handoffStatusLabel,
  handoffTerminalStatus,
} from './takeover'

describe('browser download takeover helpers', () => {
  it('matches same-URL early takeovers only to the DownloadItem tab after chain loss', () => {
    const entry = (requestId: string, tabId: number): EarlyBrowserTakeover => ({
      requestId,
      startedAt: 1,
      urls: ['https://cdn.test/shared.zip'],
      tabId,
      frameId: 0,
      promise: Promise.resolve(null),
    })
    const fromTabOne = entry('request-one', 1)
    const fromTabTwo = entry('request-two', 2)
    fromTabTwo.frameId = 4
    const entries = [fromTabOne, fromTabTwo]

    expect(findEarlyBrowserTakeoverByUrl(
      ['https://cdn.test/shared.zip'],
      { tabId: 2, frameId: 4 },
      entries,
    )).toBe(fromTabTwo)
    expect(findEarlyBrowserTakeoverByUrl(
      ['https://cdn.test/shared.zip'],
      { tabId: 2, frameId: 5 },
      entries,
    )).toBeUndefined()
    expect(findEarlyBrowserTakeoverByUrl(
      ['https://cdn.test/shared.zip'],
      { tabId: 3 },
      entries,
    )).toBeUndefined()
    expect(findEarlyBrowserTakeoverByUrl(
      ['https://cdn.test/shared.zip'],
      {},
      entries,
    )).toBeUndefined()
    expect(findEarlyBrowserTakeoverByUrl(
      ['https://cdn.test/shared.zip'],
      { tabId: 2, frameId: 4 },
      [fromTabTwo, { ...entry('request-three', 2), frameId: 4 }],
    )).toBeUndefined()
  })

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

  it('accepts a handoff even when only a confirmation window opened', () => {
    expect(desktopAcceptedHandoff({
      ok: true,
      handoff: { id: 'one', status: 'pending', presentation_ok: true, presentation_mode: 'desktop' },
    })).toBe(true)
  })

  it('maps terminal handoff statuses for popup recovery', () => {
    expect(handoffTerminalStatus('pending')).toBe(false)
    expect(handoffTerminalStatus('accepting')).toBe(false)
    expect(handoffTerminalStatus('accepted')).toBe(true)
    expect(handoffStatusLabel('accepted')).toBe('已加入')
    expect(handoffStatusLabel('canceled')).toBe('已取消')
    expect(handoffStatusLabel('expired')).toBe('已过期')
    expect(handoffStatusLabel('failed')).toBe('失败')
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
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'completed', task_downloaded_bytes: 0 }))
      .toBe('safe-to-remove')
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'failed', task_stage: 'probing', task_error_code: 'HTTP_404' }))
      .toBe('browser-fallback')
    expect(desktopTaskReadiness({ status: 'accepted', task_status: 'paused', task_downloaded_bytes: 0 }))
      .toBe('waiting')
    expect(desktopTaskReadiness({ status: 'rejected' })).toBe('browser-fallback')
  })

  it('recognises the stage names the Rust Core actually emits after a transfer', () => {
    // download_worker.rs marks the multi-segment merge "merging" and the
    // size/checksum/AV verification inside complete_payload "checking". Both are
    // only reachable once the payload is fully fetched, so both prove the
    // transfer succeeded.
    expect(desktopTaskReadiness({
      status: 'accepted',
      task_status: 'downloading',
      task_stage: 'merging',
      task_downloaded_bytes: 0,
    })).toBe('safe-to-remove')

    // The regression: a task with no known Content-Length enters "checking" with
    // downloaded_bytes still 0. The old list named 'verifying'/'verifying_checksum'
    // instead, so readiness stayed 'waiting' for the full 180 s window and
    // Chromium's duplicate item was left paused and re-polled.
    expect(desktopTaskReadiness({
      status: 'accepted',
      task_status: 'checking',
      task_stage: 'checking',
      task_downloaded_bytes: 0,
    })).toBe('safe-to-remove')

    // Stages that look plausible but do not exist must not be assumed to appear;
    // an unknown stage still means "not proven yet", which is the safe answer.
    expect(desktopTaskReadiness({
      status: 'accepted',
      task_status: 'downloading',
      task_stage: 'remuxing',
      task_downloaded_bytes: 0,
    })).toBe('waiting')

    // A pre-transfer stage must keep returning 'waiting': a one-use URL can still
    // fail during probing.
    expect(desktopTaskReadiness({
      status: 'accepted',
      task_status: 'downloading',
      task_stage: 'probing',
      task_downloaded_bytes: 0,
    })).toBe('waiting')
  })
})
