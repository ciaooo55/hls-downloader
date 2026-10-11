export const SNIFF_CURRENT_PAGE_COMMAND = 'send-current-page'

export function isSniffCurrentPageCommand(command: string): boolean {
  return command === SNIFF_CURRENT_PAGE_COMMAND
}

export function openMediaPanelCommandDispatch() {
  return {
    message: { type: 'open-media-panel', scope: 'tab' } as const,
    options: { frameId: 0 },
  }
}

export function shouldAggregateTabMedia(scope: unknown, senderFrameId: number): boolean {
  return scope === 'tab' && senderFrameId === 0
}
