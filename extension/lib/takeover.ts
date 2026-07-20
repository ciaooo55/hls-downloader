export type BrowserDownloadState = 'in_progress' | 'interrupted' | 'complete'
export type HandoffOutcome = 'desktop' | 'cancel' | 'browser'

export function browserCleanupAction(state: BrowserDownloadState): 'remove-file' | 'cancel' {
  return state === 'complete' ? 'remove-file' : 'cancel'
}

export function shouldResumeBrowserDownload(paused: boolean, handedOff: boolean): boolean {
  return paused && !handedOff
}

export function handoffOutcome(status: string): HandoffOutcome {
  if (status === 'accepted') return 'desktop'
  if (status === 'canceled') return 'cancel'
  return 'browser'
}
