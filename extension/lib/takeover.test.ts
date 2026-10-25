import { afterEach, describe, expect, it, vi } from 'vitest'
import { completeBrowserTakeover, findEarlyBrowserTakeoverByUrl, pauseBrowserDownload, refreshedDownload, type EarlyBrowserTakeover } from '../entrypoints/background'

vi.hoisted(() => {
  ;(globalThis as Record<string, unknown>).defineBackground = (main: unknown) => ({ main })
})

vi.mock('wxt/browser', () => ({
  browser: {
    contextMenus: {},
    downloads: { pause: vi.fn(), search: vi.fn(), cancel: vi.fn(), erase: vi.fn(), removeFile: vi.fn() },
    storage: {
      session: { get: vi.fn().mockResolvedValue({}), set: vi.fn().mockResolvedValue(undefined) },
      local: { get: vi.fn(), set: vi.fn() },
    },
  },
}))
import { browser } from 'wxt/browser'
import {
  browserCleanupAction,
  canContinueTakeover,
  canResumeBrowserDownload,
  desktopTaskReadiness,
  localFileTaskReadiness,
  desktopAcceptedHandoff,
  handoffTerminalStatus,
} from './takeover'

describe('browser transfer parking before desktop takeover', () => {
  const item = (overrides: Record<string, unknown> = {}) => ({
    id: 42, state: 'in_progress', paused: false, bytesReceived: 0, ...overrides,
  }) as Parameters<typeof pauseBrowserDownload>[0]

  afterEach(() => { vi.useRealTimers(); vi.unstubAllEnvs(); vi.resetAllMocks() })

  it('does not authorize a desktop transfer when Chromium refuses to pause', async () => {
    vi.stubEnv('FIREFOX', '')
    vi.mocked(browser.downloads.pause).mockRejectedValue(new Error('not pausable'))
    expect(await pauseBrowserDownload(item())).toBeNull()
  })

  it('waits for the pause operation before authorizing takeover', async () => {
    vi.stubEnv('FIREFOX', '')
    let release!: () => void
    const paused = new Promise<void>(resolve => { release = resolve })
    vi.mocked(browser.downloads.pause).mockImplementation(() => paused)
    let settled = false
    const parking = pauseBrowserDownload(item()).then(result => { settled = true; return result })
    await Promise.resolve()
    expect(settled).toBe(false)
    release()
    expect(await parking).toBe(true)
  })

  it('continues handoff when a fast file completed after the creation snapshot', async () => {
    vi.stubEnv('FIREFOX', '')
    vi.mocked(browser.downloads.pause).mockRejectedValue(new Error('already complete'))
    vi.mocked(browser.downloads.search).mockImplementation(() => Promise.resolve([item({ state: 'complete', bytesReceived: 1024 })]))
    expect(await pauseBrowserDownload(item())).toBe(false)
  })

  it('does not hand off a canceled or still-running file after pause failure', async () => {
    vi.stubEnv('FIREFOX', '')
    vi.mocked(browser.downloads.pause).mockRejectedValue(new Error('not pausable'))
    for (const state of ['interrupted', 'in_progress'] as const) {
      vi.mocked(browser.downloads.search).mockImplementation(() => Promise.resolve([item({ state })]))
      expect(await pauseBrowserDownload(item())).toBeNull()
    }
  })

  it('does not take resume ownership of a completed or already paused browser item', async () => {
    expect(await pauseBrowserDownload(item({ state: 'complete' }))).toBe(false)
    expect(await pauseBrowserDownload(item({ paused: true }))).toBe(false)
  })

  it('leaves a Firefox connection that never receives its first byte with the browser', async () => {
    vi.stubEnv('FIREFOX', 'true')
    vi.useFakeTimers()
    vi.mocked(browser.downloads.search).mockImplementation(() => Promise.resolve([item()]))
    const parking = pauseBrowserDownload(item())
    await vi.advanceTimersByTimeAsync(5_000)
    expect(await parking).toBeNull()
    expect(browser.downloads.pause).not.toHaveBeenCalled()
  })

  it('reads current download metadata without waiting for an unsupported filename event', async () => {
    vi.stubEnv('CHROME', '')
    vi.useFakeTimers()
    const current = item({ filename: 'export.zip' })
    vi.mocked(browser.downloads.search).mockImplementation(() => Promise.resolve([current]))
    let result: unknown
    const refresh = refreshedDownload(42, item()).then(value => { result = value })
    try {
      await vi.advanceTimersByTimeAsync(0)
      expect(result).toEqual(current)
    } finally {
      await vi.runAllTimersAsync()
      await refresh
    }
  })

  it('removes an acknowledged browser copy while follow-up storage is still writing', async () => {
    vi.useFakeTimers()
    let release!: () => void
    const write = new Promise<void>(resolve => { release = resolve })
    vi.mocked(browser.storage.local.get).mockImplementation(() => Promise.resolve({}))
    vi.mocked(browser.storage.session.get).mockImplementation(() => Promise.resolve({}))
    vi.mocked(browser.storage.local.set).mockImplementationOnce(() => write)
      .mockResolvedValue(undefined)
    let current = item()
    vi.mocked(browser.downloads.search).mockImplementation(() => Promise.resolve([current]))
    vi.mocked(browser.downloads.cancel).mockImplementation(async () => {
      current = item({ state: 'interrupted' })
    })
    vi.mocked(browser.downloads.erase).mockImplementation(() => Promise.resolve([42]))
    const cleanup = completeBrowserTakeover(current, 'slow-storage-handoff')
    try {
      await vi.advanceTimersByTimeAsync(0)
      expect(browser.downloads.erase).toHaveBeenCalledWith({ id: 42 })
    } finally {
      release()
      await cleanup
      vi.clearAllTimers()
    }
  })
})

describe('browser download takeover helpers', () => {
  it('retains the local source through partial copying, verification and failure', () => {
    expect(localFileTaskReadiness({ status: 'accepted', task_status: 'downloading', task_downloaded_bytes: 1024 })).toBe('waiting')
    expect(localFileTaskReadiness({ status: 'accepted', task_status: 'downloading', task_stage: 'checking' })).toBe('waiting')
    expect(localFileTaskReadiness({ status: 'accepted', task_status: 'failed', task_downloaded_bytes: 1024 })).toBe('browser-fallback')
    expect(localFileTaskReadiness({ status: 'rejected' })).toBe('browser-fallback')
    expect(localFileTaskReadiness({ status: 'accepted', task_status: 'completed' })).toBe('safe-to-remove')
  })
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

  it('distinguishes terminal handoff statuses from reconnectable observations', () => {
    expect(handoffTerminalStatus('pending')).toBe(false)
    expect(handoffTerminalStatus('accepting')).toBe(false)
    expect(handoffTerminalStatus('accepted')).toBe(true)
    // connection_lost is a local transient observation; the desktop may
    // reconnect and report the real accepted/rejected state afterwards.
    expect(handoffTerminalStatus('connection_lost')).toBe(false)
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
