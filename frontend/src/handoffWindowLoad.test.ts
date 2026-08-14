import { describe, expect, it, vi } from 'vitest'
import { loadHandoffPresentation, pendingHandoffCount } from './handoffWindowLoad'

describe('loadHandoffPresentation', () => {
  it('paints the offer before settings or the pending queue return', async () => {
    const painted: string[] = []
    let releaseSettings: () => void = () => {}
    const settingsGate = new Promise<void>(resolve => { releaseSettings = resolve })

    const pending = loadHandoffPresentation(
      'offer-1',
      {
        fetchHandoff: async () => ({ id: 'offer-1', status: 'pending', filename: 'setup.exe' }),
        fetchSettings: async () => {
          await settingsGate
          painted.push('settings')
          return { download_dir: 'D:\\Downloads' }
        },
        fetchHandoffs: async () => {
          await settingsGate
          painted.push('queue')
          return [
            { id: 'offer-1', status: 'pending', filename: 'setup.exe' },
            { id: 'offer-2', status: 'pending', filename: 'other.exe' },
          ]
        },
      },
      {
        item: item => painted.push(`item:${item.filename}`),
        extras: state => painted.push(`queue:${state.queueRemaining}`),
      },
    )

    await vi.waitFor(() => expect(painted).toEqual(['item:setup.exe']))
    releaseSettings()
    await expect(pending).resolves.toEqual({ close: false })
    expect(painted[0]).toBe('item:setup.exe')
    expect(painted).toContain('settings')
    expect(painted).toContain('queue')
    expect(painted[painted.length - 1]).toBe('queue:1')
  })

  it('closes instead of painting an already resolved offer', async () => {
    const painted: string[] = []
    const result = await loadHandoffPresentation(
      'offer-1',
      {
        fetchHandoff: async () => ({ id: 'offer-1', status: 'accepted' }),
        fetchSettings: async () => {
          painted.push('settings')
          return {}
        },
        fetchHandoffs: async () => [],
      },
      {
        item: () => painted.push('item'),
        extras: () => painted.push('extras'),
      },
    )
    expect(result).toEqual({ close: true })
    expect(painted).toEqual([])
  })
})

describe('pendingHandoffCount', () => {
  it('ignores the visible offer and non-pending rows', () => {
    expect(pendingHandoffCount('a', [
      { id: 'a', status: 'pending' },
      { id: 'b', status: 'pending' },
      { id: 'c', status: 'accepted' },
    ])).toBe(1)
  })
})
