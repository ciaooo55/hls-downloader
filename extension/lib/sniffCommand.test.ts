import { describe, expect, it } from 'vitest'
import { isSniffCurrentPageCommand, openMediaPanelMessage, SNIFF_CURRENT_PAGE_COMMAND } from './sniffCommand'

describe('sniff current page command', () => {
  it('maps Ctrl+Shift+Y to the overlay open message', () => {
    expect(SNIFF_CURRENT_PAGE_COMMAND).toBe('send-current-page')
    expect(isSniffCurrentPageCommand('send-current-page')).toBe(true)
    expect(isSniffCurrentPageCommand('open-media-panel')).toBe(false)
    expect(openMediaPanelMessage()).toEqual({ type: 'open-media-panel' })
  })
})
