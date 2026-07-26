import { describe, expect, it } from 'vitest'
import {
  applyTheme,
  normalizeThemePreference,
  resolveTheme,
  THEME_BASE_CSS,
  THEME_TOKENS_CSS,
} from './theme'

describe('theme resolution', () => {
  it('resolves auto against the system scheme and honors explicit choices', () => {
    expect(resolveTheme('auto', true)).toBe('dark')
    expect(resolveTheme('auto', false)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
    expect(resolveTheme('light', true)).toBe('light')
  })

  it('normalizes stored values defensively', () => {
    expect(normalizeThemePreference('dark')).toBe('dark')
    expect(normalizeThemePreference('light')).toBe('light')
    expect(normalizeThemePreference('legacy')).toBe('auto')
    expect(normalizeThemePreference(undefined)).toBe('auto')
  })
})

function fakeRoot(): HTMLElement {
  const attributes = new Map<string, string>()
  return {
    setAttribute: (name: string, value: string) => void attributes.set(name, value),
    getAttribute: (name: string) => attributes.get(name) ?? null,
  } as unknown as HTMLElement
}

describe('applyTheme', () => {
  function fakeMedia(matches: boolean) {
    const listeners = new Set<() => void>()
    const media = {
      matches,
      addEventListener: (_type: string, listener: () => void) => listeners.add(listener),
      removeEventListener: (_type: string, listener: () => void) => listeners.delete(listener),
      flip() {
        media.matches = !media.matches
        listeners.forEach(listener => listener())
      },
      listenerCount: () => listeners.size,
    }
    return media
  }

  it('stamps the resolved theme and follows system changes in auto mode', () => {
    const root = fakeRoot()
    const media = fakeMedia(true)
    const cleanup = applyTheme(root, 'auto', () => media as unknown as MediaQueryList)
    expect(root.getAttribute('data-hlsd-theme')).toBe('dark')
    media.flip()
    expect(root.getAttribute('data-hlsd-theme')).toBe('light')
    cleanup()
    expect(media.listenerCount()).toBe(0)
  })

  it('does not track the system scheme for explicit preferences', () => {
    const root = fakeRoot()
    const media = fakeMedia(false)
    applyTheme(root, 'dark', () => media as unknown as MediaQueryList)
    expect(root.getAttribute('data-hlsd-theme')).toBe('dark')
    media.flip()
    expect(root.getAttribute('data-hlsd-theme')).toBe('dark')
    expect(media.listenerCount()).toBe(0)
  })
})

describe('token sheets', () => {
  it('defines both themes with the same token set', () => {
    const names = (block: string) =>
      [...block.matchAll(/--[a-z0-9-]+(?=:)/g)].map(match => match[0]).sort()
    const dark = THEME_TOKENS_CSS.match(/\[data-hlsd-theme="dark"\]\{[^}]+\}/)![0]
    const light = THEME_TOKENS_CSS.match(/\[data-hlsd-theme="light"\]\{[^}]+\}/)![0]
    expect(names(dark)).toEqual(names(light))
    expect(names(dark)).toContain('--primary')
  })

  it('keeps base primitives free of hard-coded palette colors', () => {
    // Base components must derive every color from tokens so both themes work;
    // #fff on the solid primary button is the single deliberate exception.
    const colors = THEME_BASE_CSS.match(/#(?!fff\b)[0-9a-f]{3,8}\b/gi) || []
    expect(colors).toEqual([])
  })
})
