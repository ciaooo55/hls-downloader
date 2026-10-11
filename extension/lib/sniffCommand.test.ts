import { describe, expect, it } from 'vitest'
import { isSniffCurrentPageCommand, openMediaPanelCommandDispatch, shouldAggregateTabMedia, SNIFF_CURRENT_PAGE_COMMAND } from './sniffCommand'

describe('sniff current page command', () => {
  it('maps Ctrl+Shift+Y to the overlay open message', () => {
    expect(SNIFF_CURRENT_PAGE_COMMAND).toBe('send-current-page')
    expect(isSniffCurrentPageCommand('send-current-page')).toBe(true)
    expect(isSniffCurrentPageCommand('open-media-panel')).toBe(false)
    expect(openMediaPanelCommandDispatch()).toEqual({
      message: { type: 'open-media-panel', scope: 'tab' },
      options: { frameId: 0 },
    })
  })

  it('aggregates tab resources only for an explicit top-frame panel request', () => {
    expect(shouldAggregateTabMedia('tab', 0)).toBe(true)
    expect(shouldAggregateTabMedia('tab', 3)).toBe(false)
    expect(shouldAggregateTabMedia(undefined, 0)).toBe(false)
  })
})
