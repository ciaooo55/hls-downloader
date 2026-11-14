import type { DownloadClickIntent, MediaResource } from './resources'

export interface EarlyBrowserTakeover {
  requestId: string
  startedAt: number
  urls: string[]
  tabId: number
  frameId: number
  promise: Promise<{ resource: MediaResource, intent?: DownloadClickIntent, bypass?: boolean } | null>
}
/**
 * Locate an early takeover when the request chain is no longer available.
 * Tab navigation events routinely clear the chain between onHeadersReceived
 * and downloads.onCreated. Some browsers expose the originating tab on the
 * DownloadItem; only a unique URL-and-tab identity match is safe to reuse.
 */
export function validDownloadIdentity(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) >= 0
}

export function findEarlyBrowserTakeoverByUrl(
  candidates: Array<string | undefined>,
  downloadIdentity: { tabId?: unknown, frameId?: unknown },
  entries: Iterable<EarlyBrowserTakeover>,
): EarlyBrowserTakeover | undefined {
  const wanted = candidates.filter((value): value is string => Boolean(value))
  const tabId = downloadIdentity.tabId
  if (!wanted.length || !validDownloadIdentity(tabId)) return undefined
  const frameId = downloadIdentity.frameId
  let match: EarlyBrowserTakeover | undefined
  for (const entry of entries) {
    if (entry.tabId !== tabId) continue
    if (validDownloadIdentity(frameId) && validDownloadIdentity(entry.frameId) && entry.frameId !== frameId) continue
    if (!entry.urls.some(url => wanted.includes(url))) continue
    if (match) return undefined
    match = entry
  }
  return match
}

export type BrowserDownloadState = 'in_progress' | 'interrupted' | 'complete'

export function browserCleanupAction(state: BrowserDownloadState): 'remove-file' | 'cancel' {
  return state === 'complete' ? 'remove-file' : 'cancel'
}

export function canContinueTakeover(state: BrowserDownloadState, paused = false): boolean {
  // Chromium may briefly report a download as interrupted immediately after
  // downloads.pause(). In that case the item is still resumable and must be
  // inspected for takeover. An unpaused interrupted item remains a genuine
  // failure/cancellation and is not eligible.
  return state === 'in_progress' || state === 'complete' || (paused && state === 'interrupted')
}

/** A paused Chromium item can transiently become interrupted and remain resumable. */
export function canResumeBrowserDownload(state: BrowserDownloadState): boolean {
  return state === 'in_progress' || state === 'interrupted'
}

export interface BrowserHandoffPayload {
  id?: string
  status?: string
  presented?: boolean
  presentation?: string
  presentation_mode?: string
  presentation_ok?: boolean
  presentation_queued?: boolean
  presentation_error?: string
  task_id?: string
  task_status?: string
  task_stage?: string
  task_downloaded_bytes?: number
  task_total_bytes?: number
  task_error_code?: string
}

export type DesktopTaskReadiness = 'waiting' | 'safe-to-remove' | 'browser-fallback'

export function localFileTaskReadiness(handoff: BrowserHandoffPayload): DesktopTaskReadiness {
  const readiness = desktopTaskReadiness(handoff)
  if (readiness === 'browser-fallback') return readiness
  // 本地文件是 Core 复制的源；即使已开始复制，也必须等最终发布成功才能删除。
  return handoff.status === 'accepted' && ['done', 'completed'].includes(handoff.task_status || '')
    ? 'safe-to-remove' : 'waiting'
}

/**
 * Decide whether Chromium may discard its paused DownloadItem. Handoff
 * acceptance proves only that a task was created; it does not prove that a
 * short-lived/one-use request can be replayed by the desktop downloader.
 */
export function desktopTaskReadiness(handoff: BrowserHandoffPayload): DesktopTaskReadiness {
  const handoffStatus = String(handoff.status || '')
  if (['canceled', 'rejected', 'expired', 'failed'].includes(handoffStatus)) return 'browser-fallback'
  if (handoffStatus !== 'accepted') return 'waiting'

  const taskStatus = String(handoff.task_status || '')
  if (['failed', 'canceled', 'unsupported'].includes(taskStatus)) return 'browser-fallback'
  if (taskStatus === 'done' || taskStatus === 'completed') return 'safe-to-remove'
  if (Math.max(0, Number(handoff.task_downloaded_bytes || 0)) > 0) return 'safe-to-remove'

  // Merging/checking can only be reached after the payload is fully fetched, so
  // they are the stages that prove the transfer succeeded. Do not use
  // status=downloading alone: HTTP tasks enter that status before the probing
  // request, which is exactly where a one-use URL can still fail.
  //
  // The list must match the stage names the Rust Core actually emits
  // (download_worker.rs): "merging" for the multi-segment merge, and "checking"
  // for the size/checksum/AV verification inside complete_payload. Names that
  // look plausible but do not exist ("remuxing", "verifying",
  // "verifying_checksum") previously made a task with no known Content-Length
  // sit in "checking" with downloaded_bytes=0 and never reach
  // safe-to-remove, burning the whole readiness window and leaving Chromium's
  // duplicate item paused.
  if (['merging', 'checking'].includes(String(handoff.task_stage || ''))) {
    return 'safe-to-remove'
  }
  return 'waiting'
}
export function desktopAcceptedHandoff(response: unknown): boolean {
  if (!response || typeof response !== 'object') return false
  const value = response as { ok?: boolean; handoff?: BrowserHandoffPayload }
  if (value.ok !== true || !value.handoff?.id) return false
  // A recovered user cancellation still proves durable admission. Resuming
  // the browser here would undo that decision and start another download.
  if (value.handoff.status === 'failed') return false
  if (value.handoff.presentation_ok === false) return false
  if (value.handoff.presentation === 'failed') return false
  const mode = value.handoff.presentation_mode
  if (mode === 'none') return false
  return true
}

export function handoffTerminalStatus(status?: string): boolean {
  return ['accepted', 'canceled', 'rejected', 'expired', 'failed'].includes(status || '')
}
