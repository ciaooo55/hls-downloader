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

export function desktopAcceptedHandoff(response: unknown): boolean {
  if (!response || typeof response !== 'object') return false
  const value = response as { ok?: boolean; handoff?: { id?: string } }
  return value.ok === true && Boolean(value.handoff?.id)
}
