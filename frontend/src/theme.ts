export type Theme = 'dark' | 'light'
export type ThemePreference = Theme | 'system'

export function resolveThemePreference(saved: string | null): ThemePreference {
  return saved === 'dark' || saved === 'light' ? saved : 'system'
}

export function resolveTheme(saved: string | null, systemDark: boolean): Theme {
  if (saved === 'dark' || saved === 'light') return saved
  return systemDark ? 'dark' : 'light'
}
