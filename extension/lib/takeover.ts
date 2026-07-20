export type BrowserDownloadState = 'in_progress' | 'interrupted' | 'complete'

export function browserCleanupAction(state: BrowserDownloadState): 'remove-file' | 'cancel' {
  return state === 'complete' ? 'remove-file' : 'cancel'
}

export function shouldResumeBrowserDownload(paused: boolean, handedOff: boolean): boolean {
  return paused && !handedOff
}
