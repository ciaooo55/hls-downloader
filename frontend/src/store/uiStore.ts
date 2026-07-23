import { create } from 'zustand'
import type { TaskFilter } from '@/components/Sidebar'
import { resolveTheme, resolveThemePreference, type Theme, type ThemePreference } from '@/theme'

interface UiState {
  filter: TaskFilter
  query: string
  themePreference: ThemePreference
  systemDark: boolean
  setFilter: (filter: TaskFilter) => void
  setQuery: (query: string) => void
  setThemePreference: (preference: ThemePreference) => void
  setSystemDark: (value: boolean) => void
  toggleTheme: () => void
}

function readStoredTheme(): string | null {
  try {
    if (typeof globalThis.localStorage === 'undefined') return null
    const storage = globalThis.localStorage as Storage | undefined
    if (!storage || typeof storage.getItem !== 'function') return null
    return storage.getItem('hls_theme')
  } catch {
    return null
  }
}

function writeStoredTheme(preference: string) {
  try {
    if (typeof globalThis.localStorage === 'undefined') return
    const storage = globalThis.localStorage as Storage | undefined
    if (!storage || typeof storage.setItem !== 'function') return
    storage.setItem('hls_theme', preference)
  } catch {
    // ignore storage failures in restricted environments
  }
}

const initialPreference = resolveThemePreference(readStoredTheme())

export const useUiStore = create<UiState>((set, get) => ({
  filter: 'all',
  query: '',
  themePreference: initialPreference,
  systemDark: typeof matchMedia !== 'undefined' ? matchMedia('(prefers-color-scheme: dark)').matches : true,
  setFilter: filter => set({ filter }),
  setQuery: query => set({ query }),
  setThemePreference: preference => {
    writeStoredTheme(preference)
    set({ themePreference: preference })
  },
  setSystemDark: value => set({ systemDark: value }),
  toggleTheme: () => {
    const current = resolveTheme(get().themePreference, get().systemDark)
    get().setThemePreference(current === 'dark' ? 'light' : 'dark')
  },
}))

export function selectTheme(state: Pick<UiState, 'themePreference' | 'systemDark'>): Theme {
  return resolveTheme(state.themePreference, state.systemDark)
}
