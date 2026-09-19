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
    expect(dark).toContain('--bg:#151719')
    expect(dark).toContain('--surface:#1c1f23')
    expect(dark).toContain('--primary:#5ea2f3')
    expect(light).toContain('--border:#d8e0ea')
  })

  it('keeps base primitives free of hard-coded palette colors', () => {
    // Base components must derive every color from tokens so both themes work.
    const colors = THEME_BASE_CSS.match(/#(?!fff\b)[0-9a-f]{3,8}\b/gi) || []
    expect(colors).toEqual([])
  })
})

// ---------------------------------------------------------------------------
// Contrast guards
//
// The dark theme's --primary is a *light* blue because it is used far more
// often as ink (icons, text, focus rings) than as a fill. White text on it is
// only 2.65:1, so --on-primary must be the dark ink there. These tests lock
// that in: reusing one token for both roles is exactly how this regressed.
// ---------------------------------------------------------------------------
function themeTokens(theme: 'dark' | 'light'): Record<string, string> {
  const block = THEME_TOKENS_CSS.match(
    new RegExp(`\\[data-hlsd-theme="${theme}"\\]\\{([^}]+)\\}`),
  )![1]
  const out: Record<string, string> = {}
  for (const [, name, value] of block.matchAll(/--([\w-]+):([^;]+);/g)) out[name] = value.trim()
  return out
}

function rgb(hex: string): [number, number, number] {
  const h = hex.replace('#', '')
  const full = h.length === 3 ? [...h].map(c => c + c).join('') : h
  return [0, 2, 4].map(i => parseInt(full.slice(i, i + 2), 16)) as [number, number, number]
}

function luminance(hex: string): number {
  const channel = (v: number) => {
    const s = v / 255
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  const [r, g, b] = rgb(hex)
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

/** color-mix(in srgb, a pctA%, b) -> hex, matching the browser's mix. */
function mix(a: string, b: string, pctA: number): string {
  const [ra, ga, ba] = rgb(a)
  const [rb, gb, bb] = rgb(b)
  const f = pctA / 100
  const to = (x: number, y: number) => Math.round(x * f + y * (1 - f))
  return (
    '#' +
    [to(ra, rb), to(ga, gb), to(ba, bb)]
      .map(v => v.toString(16).padStart(2, '0'))
      .join('')
  )
}

describe('contrast guards', () => {
  for (const theme of ['dark', 'light'] as const) {
    const t = themeTokens(theme)

    it(`${theme}: on-primary stays readable on every filled control`, () => {
      // WCAG 2.1 AA for body text is 4.5:1; these are 10-13px labels.
      expect(contrast(t['on-primary'], t['primary'])).toBeGreaterThanOrEqual(4.5)
      expect(contrast(t['on-primary'], t['primary-hover'])).toBeGreaterThanOrEqual(4.5)
      // .download.push-tv / .download.cast fill with the accent on hover.
      expect(contrast(t['on-primary'], t['purple'])).toBeGreaterThanOrEqual(4.5)
      expect(contrast(t['on-primary'], t['green'])).toBeGreaterThanOrEqual(4.5)
    })

    it(`${theme}: accent ink stays readable on the canvas`, () => {
      // --primary doubles as icon/text ink, so it must work on both surfaces.
      expect(contrast(t['primary'], t['bg'])).toBeGreaterThanOrEqual(4.5)
      expect(contrast(t['primary'], t['surface'])).toBeGreaterThanOrEqual(4.5)
      expect(contrast(t['green'], t['surface'])).toBeGreaterThanOrEqual(4.5)
    })

    it(`${theme}: accent ink stays readable on a tint of its own colour`, () => {
      // A tint lifts the background *towards* the accent, which eats exactly the
      // contrast the plain accent was tuned for (worst case was 3.41:1). Each
      // --*-ink is the minimum shift away from the tint that clears AA, so these
      // are pinned against the literal backgrounds used in the CSS.
      expect(contrast(t['primary-ink'], mix(t['primary'], t['surface-2'], 20))).toBeGreaterThanOrEqual(4.5) // .hlsd-badge
      expect(contrast(t['primary-ink'], mix(t['primary'], t['surface'], 10))).toBeGreaterThanOrEqual(4.5) // .empty-icon, .scan-button:hover
      expect(contrast(t['green-ink'], mix(t['green'], t['surface-3'], 16))).toBeGreaterThanOrEqual(4.5) // .hlsd-button.active
      expect(contrast(t['green-ink'], mix(t['green'], t['surface-3'], 22))).toBeGreaterThanOrEqual(4.5) // .hlsd-button.active:hover
      expect(contrast(t['green-ink'], mix(t['green'], t['surface'], 14))).toBeGreaterThanOrEqual(4.5) // .result
      expect(contrast(t['green-ink'], mix(t['green'], t['surface-3'], 18))).toBeGreaterThanOrEqual(4.5) // .pin.active
      expect(contrast(t['red-ink'], mix(t['red'], t['surface'], 12))).toBeGreaterThanOrEqual(4.5) // .result.error
      expect(contrast(t['red-ink'], mix(t['red'], t['surface'], 10))).toBeGreaterThanOrEqual(4.5) // .send-error
      expect(contrast(t['purple-ink'], mix(t['purple'], t['surface-3'], 22))).toBeGreaterThanOrEqual(4.5) // .push-button
      expect(contrast(t['purple-ink'], mix(t['purple'], t['surface-3'], 22))).toBeGreaterThanOrEqual(4.5) // .cast-button
    })
  }

  it('every accent has a dedicated tint ink, and it is not the plain accent', () => {
    for (const theme of ['dark', 'light'] as const) {
      const t = themeTokens(theme)
      for (const hue of ['primary', 'green', 'red', 'purple']) {
        expect(t[`${hue}-ink`], `${theme}: --${hue}-ink missing`).toBeTruthy()
        // Collapsing the ink back onto the accent is the exact regression the
        // tinted pills hit; keeping them distinct is what makes it fail loudly.
        expect(t[`${hue}-ink`], `${theme}: --${hue}-ink collapsed into --${hue}`).not.toBe(t[hue])
      }
    }
  })

  for (const theme of ['dark', 'light'] as const) {
    it(`${theme}: control boundaries clear 3:1 (WCAG 1.4.11 non-text)`, () => {
      const t = themeTokens(theme)
      // --control-border exists so a <select> reads as an input rather than as a
      // line of text. Reusing the separator token --border gave 1.24-1.51:1, which
      // is the right answer for a divider and the wrong answer for a control.
      // --surface-3 is the binding constraint, so it is asserted explicitly.
      for (const surface of ['surface-2', 'surface', 'surface-3', 'bg'] as const) {
        expect(
          contrast(t['control-border'], t[surface]),
          `${theme}: control-border on ${surface}`,
        ).toBeGreaterThanOrEqual(3.0)
      }
      // A separator must stay a separator -- if this ever reaches 3:1 the two
      // roles have been collapsed and every divider in the UI got much louder.
      expect(contrast(t['border'], t['surface'])).toBeLessThan(3.0)
    })
  }

  it('dark and light deliberately use different on-primary inks', () => {
    // Not a cosmetic difference: #2563eb needs white, #5ea2f3 needs dark ink.
    expect(themeTokens('dark')['on-primary']).not.toBe(themeTokens('light')['on-primary'])
    expect(themeTokens('light')['on-primary']).toBe('#ffffff')
  })
})
