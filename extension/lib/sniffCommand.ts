export const SNIFF_CURRENT_PAGE_COMMAND = 'send-current-page'

export function isSniffCurrentPageCommand(command: string): boolean {
  return command === SNIFF_CURRENT_PAGE_COMMAND
}

export function openMediaPanelMessage(): { type: 'open-media-panel' } {
  return { type: 'open-media-panel' }
}
