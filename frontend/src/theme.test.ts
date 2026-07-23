import { describe, expect, it } from 'vitest'
import { resolveTheme, resolveThemePreference } from './theme'

describe('resolveTheme', () => {
  it('uses the system theme until the user chooses an override', () => {
    expect(resolveTheme(null, true)).toBe('dark')
    expect(resolveTheme(null, false)).toBe('light')
  })

  it('prefers the saved user theme', () => {
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
  })

  it('keeps system as an explicit preference', () => {
    expect(resolveThemePreference(null)).toBe('system')
    expect(resolveThemePreference('system')).toBe('system')
    expect(resolveThemePreference('light')).toBe('light')
    expect(resolveThemePreference('dark')).toBe('dark')
  })
})
