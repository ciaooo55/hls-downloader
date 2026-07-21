export type BrowserDownloadState = 'in_progress' | 'interrupted' | 'complete'

export function browserCleanupAction(state: BrowserDownloadState): 'remove-file' | 'cancel' {
  return state === 'complete' ? 'remove-file' : 'cancel'
}

export function shouldResumeBrowserDownload(paused: boolean, handedOff: boolean): boolean {
  return paused && !handedOff
}

export function canContinueTakeover(paused: boolean, state: BrowserDownloadState): boolean {
  return paused || state === 'complete'
}

export type HandoffPresentationMode = 'desktop' | 'desktop-pending' | 'ui-fallback' | 'none' | string

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
  return Boolean(status && status !== 'pending' && status !== 'accepting')
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
      return '待确认'
    default:
      return status || '待确认'
  }
}
