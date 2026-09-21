import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

/*
 * Button motion guards
 * --------------------
 * The overlay and popup used to ship buttons that either had no transition at
 * all (`.video-more`, `.hover-action`, `.scan-button`, `.empty-retry`,
 * `.copy-link`, `.quality-menu button`, `.update-notice button`) or declared a
 * `transform` transition that no state ever changed — `.video-download` listed
 * `transform .16s` while `:hover` only touched the background and `:active`
 * only swapped the cursor. Every click therefore read as a hard colour cut.
 *
 * These tests pin the fix: every interactive button must declare a transition,
 * must change `transform` on press, and both motion surfaces must honour
 * `prefers-reduced-motion` by disabling transitions *and* animations.
 */

const extensionRoot = fileURLToPath(new URL('../', import.meta.url))
const content = readFileSync(`${extensionRoot}/entrypoints/content.ts`, 'utf8')
const popupCss = readFileSync(`${extensionRoot}/entrypoints/popup/style.css`, 'utf8')

/** Extract the shadow-DOM stylesheet the content script installs. */
function overlayCss(): string {
  const start = content.indexOf('.video-buttons{position:fixed')
  expect(start, 'overlay stylesheet marker').toBeGreaterThan(-1)
  return content.slice(start, content.indexOf('`', start))
}

/**
 * selector -> joined declarations. Rules are split first and the selector list
 * second, so an entry sitting in the middle of a comma-separated list (the
 * shared motion rule is one such list) is found just like a standalone one.
 * Repeated selectors accumulate, matching how the cascade merges them.
 */
function rules(css: string): Map<string, string> {
  const out = new Map<string, string>()
  // Strip comments first: a comment directly above a rule would otherwise be
  // glued onto that rule's selector text and break the lookup.
  const sheet = css.replace(/\/\*[\s\S]*?\*\//g, ' ')
  for (const match of sheet.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    for (const selector of match[1].split(',')) {
      const key = selector.trim()
      if (!key) continue
      out.set(key, `${out.get(key) ?? ''} ${match[2]}`)
    }
  }
  return out
}

/**
 * True when any rule for `selector` matches `property`. Extra pseudo classes
 * count, so `.scan-button:active` also matches the real selector
 * `.scan-button:active:not(:disabled)`.
 */
function declares(css: string, selector: string, property: RegExp): boolean {
  for (const [key, body] of rules(css)) {
    const isRule = key === selector || [':', '.', ' '].some(sep => key.startsWith(`${selector}${sep}`))
    if (isRule && property.test(body)) return true
  }
  return false
}

describe('overlay button motion', () => {
  const css = overlayCss()
  // One shared rule drives the transition for every overlay control; the
  // per-state rules below then say what actually moves.
  const sharedTransition = '.video-download,.video-more,.hover-action,.pin,.close,.download{transition:'

  it('gives every overlay control a transition', () => {
    // `.video-more` and `.hover-action` had none at all, and `.video-download`
    // transitioned `transform` while no state ever changed it.
    expect(css).toContain(sharedTransition)
  })

  it('moves the download button on hover and on press', () => {
    expect(declares(css, '.video-download:hover', /transform:\s*translateY\(-1px\)/)).toBe(true)
    expect(declares(css, '.video-download:active', /transform:\s*scale\(/)).toBe(true)
  })

  it('moves the more button and the hover actions on press', () => {
    expect(declares(css, '.video-more:active', /transform:\s*scale\(/)).toBe(true)
    expect(declares(css, '.hover-action:active', /transform:\s*scale\(/)).toBe(true)
  })

  it('moves the panel buttons on press', () => {
    expect(declares(css, '.pin:active', /transform:\s*scale\(/)).toBe(true)
    expect(declares(css, '.close:active', /transform:\s*scale\(/)).toBe(true)
    expect(declares(css, '.download:active', /transform:\s*scale\(/)).toBe(true)
  })

  it('suppresses button motion while the control is being dragged', () => {
    // Dragging rewrites the group's left/top; a button transform on top of
    // that reads as the control jittering instead of following the pointer.
    expect(declares(css, '.video-action-group.dragging .video-download', /transform:\s*none/)).toBe(true)
    expect(declares(css, '.video-action-group.dragging .video-more', /transform:\s*none/)).toBe(true)
  })

  it('keeps the identifying state free of interaction feedback', () => {
    // "identifying" is aria-disabled; lifting it on hover would imply it is
    // clickable before the resource has been resolved.
    expect(declares(css, '.video-download.identifying:hover', /transform:\s*none/)).toBe(true)
  })
})

describe('popup button motion', () => {
  // Buttons styled from scratch in the popup sheet rather than inheriting
  // `.hlsd-button`, which already had a full hover/active pair.
  const bespoke = [
    '.update-notice button',
    '.scan-button',
    '.empty-retry',
    '.copy-link',
    '.quality-trigger',
    '.quality-menu button',
    '.restore-site-prompts',
  ]

  for (const selector of bespoke) {
    it(`${selector} declares a transition`, () => {
      expect(declares(popupCss, selector, /transition:/), selector).toBe(true)
    })
  }

  it('scales every pressable bespoke button on press', () => {
    // A transition with no changed property is the bug that started this;
    // each of these must actually move when pressed.
    for (const selector of bespoke) {
      if (selector === '.restore-site-prompts') continue // text link, colour only
      const active = /\s/.test(selector) ? `${selector}:active` : selector.replace(/^([.\w-]+)/, '$1:active')
      expect(declares(popupCss, active, /transform:\s*scale\(/), `${selector} has no :active transform`).toBe(true)
    }
  })

  it('keeps the shared .hlsd-button press feedback intact', () => {
    const theme = readFileSync(`${extensionRoot}/lib/theme.ts`, 'utf8')
    expect(theme).toContain('.hlsd-button:active:not(:disabled){transform:scale(.975)}')
  })
})

describe('reduced motion', () => {
  it('disables transitions and animations in both surfaces', () => {
    // The content script only killed transitions, so an animation would keep
    // running for a user who asked for less motion.
    expect(content).toContain('@media(prefers-reduced-motion:reduce){*{transition:none!important;animation:none!important}}')
    expect(popupCss).toContain('@media (prefers-reduced-motion: reduce) { * { animation: none !important; transition: none !important; } }')
  })
})
