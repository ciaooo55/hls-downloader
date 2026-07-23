import { beforeEach, describe, expect, it } from 'vitest'
import { selectTheme, useUiStore } from './uiStore'

describe('uiStore', () => {
  beforeEach(() => {
    useUiStore.setState({
      filter: 'all',
      query: '',
      themePreference: 'dark',
      systemDark: true,
    })
  })

  it('updates filter and query', () => {
    useUiStore.getState().setFilter('running')
    useUiStore.getState().setQuery('movie')
    expect(useUiStore.getState().filter).toBe('running')
    expect(useUiStore.getState().query).toBe('movie')
  })

  it('resolves theme from preference and system', () => {
    useUiStore.setState({ themePreference: 'system', systemDark: false })
    expect(selectTheme(useUiStore.getState())).toBe('light')
    useUiStore.setState({ systemDark: true })
    expect(selectTheme(useUiStore.getState())).toBe('dark')
  })

  it('toggles between light and dark preferences', () => {
    useUiStore.setState({ themePreference: 'dark', systemDark: true })
    useUiStore.getState().toggleTheme()
    expect(useUiStore.getState().themePreference).toBe('light')
    useUiStore.getState().toggleTheme()
    expect(useUiStore.getState().themePreference).toBe('dark')
  })
})
