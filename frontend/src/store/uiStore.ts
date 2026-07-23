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
  theme: () => Theme
  toggleTheme: () => void
}

const initialPreference = resolveThemePreference(
  typeof localStorage !== 'undefined' ? localStorage.getItem('hls_theme') : null,
)

export const useUiStore = create<UiState>((set, get) => ({
  filter: 'all',
  query: '',
  themePreference: initialPreference,
  systemDark: typeof matchMedia !== 'undefined' ? matchMedia('(prefers-color-scheme: dark)').matches : true,
  setFilter: filter => set({ filter }),
  setQuery: query => set({ query }),
  setThemePreference: preference => {
    localStorage.setItem('hls_theme', preference)
    set({ themePreference: preference })
  },
  setSystemDark: value => set({ systemDark: value }),
  theme: () => resolveTheme(get().themePreference, get().systemDark),
  toggleTheme: () => {
    const next = get().theme() === 'dark' ? 'light' : 'dark'
    get().setThemePreference(next)
  },
}))
