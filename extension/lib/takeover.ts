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

export type HandoffPresentationMode = 'native-shell' | 'native-shell-pending' | 'desktop' | 'desktop-pending' | 'ui-fallback' | 'none' | string

export interface BrowserHandoffPayload {
  id?: string
  status?: string
  presented?: boolean
  presentation?: string
  presentation_mode?: HandoffPresentationMode
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

  // Merging/verifying can only be reached after the transfer succeeded. Do
  // not use status=downloading alone: HTTP tasks enter that status before the
  // probing request, which is exactly where a one-use URL can still fail.
  if (['merging', 'remuxing', 'verifying', 'verifying_checksum'].includes(String(handoff.task_stage || ''))) {
    return 'safe-to-remove'
  }
  return 'waiting'
}

/** Presentation of a confirmation window is not ownership of the transfer. */
export function mayDiscardBrowserTransfer(
  handoffStatus?: string,
  readiness?: DesktopTaskReadiness,
): boolean {
  return handoffStatus === 'accepted' && readiness === 'safe-to-remove'
}

export function desktopAcceptedHandoff(response: unknown): boolean {
  if (!response || typeof response !== 'object') return false
  const value = response as { ok?: boolean; handoff?: BrowserHandoffPayload }
  if (value.ok !== true || !value.handoff?.id) return false
  if (value.handoff.presentation_ok === false) return false
  if (value.handoff.presentation === 'failed') return false
  const mode = value.handoff.presentation_mode
  if (mode === 'none') return false
  return true
}

export function handoffTerminalStatus(status?: string): boolean {
  return ['accepted', 'canceled', 'rejected', 'expired', 'failed'].includes(status || '')
}

export function handoffStatusLabel(status?: string): string {
  switch (status) {
    case 'accepted':
      return '已加入'
    case 'canceled':
    case 'rejected':
      return '已取消'
    case 'expired':
      return '已过期'
    case 'accepting':
      return '确认中'
    case 'pending':
      return '等待确认'
    case 'connection_lost':
      return '连接中断'
    default:
      return status || '待确认'
  }
}
