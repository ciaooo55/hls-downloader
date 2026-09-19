/**
 * Shared design tokens for every extension surface (popup and in-page
 * shadow-DOM panels), aligned with the desktop app's Cockpit-style palette
 * so the browser surface and Compose workbench read as one product family.
 *
 * Theme resolution: the stored preference is 'auto' | 'dark' | 'light'.
 * 'auto' is resolved against prefers-color-scheme in JS and a concrete
 * `data-hlsd-theme` attribute is stamped on the surface root, so the same
 * token block works identically in documents and shadow roots.
 */

export type ThemePreference = 'auto' | 'dark' | 'light'
export type ResolvedTheme = 'dark' | 'light'

export const THEME_STORAGE_KEY = 'themePreference'

export const THEME_TOKENS_CSS = `
[data-hlsd-theme="dark"]{
  color-scheme:dark;
  --bg:#151719;--surface:#1c1f23;--surface-2:#23272b;--surface-3:#2b3035;
  --border:#383d43;--control-border:#767a7e;--text:#f4f5f6;--muted:#c5c9cf;--faint:#969ca4;
  --primary:#5ea2f3;--primary-hover:#7eb7f6;--on-primary:#151719;
  --green:#22c55e;--amber:#f59e0b;--red:#ef4444;--purple:#a78bfa;
  --primary-ink:#7eb7f6;--green-ink:#55d283;--red-ink:#f27070;--purple-ink:#c4b5fd;
  --shadow:rgba(0,0,0,.35);--rail:#1c1f23;
  --overlay-border:#464c53;
  --z-extension-overlay:2147483647;--z-extension-video:2147483646;
}
[data-hlsd-theme="light"]{
  color-scheme:light;
  --bg:#eef2f6;--surface:#ffffff;--surface-2:#f5f7fa;--surface-3:#e8edf3;
  --border:#d8e0ea;--control-border:#808896;--text:#0f172a;--muted:#475569;--faint:#5f6e83;
  --primary:#2563eb;--primary-hover:#1d4ed8;--on-primary:#ffffff;
  --green:#15803d;--amber:#d97706;--red:#dc2626;--purple:#7c3aed;
  --primary-ink:#1d4ed8;--green-ink:#166534;--red-ink:#b91c1c;--purple-ink:#5b21b6;
  --shadow:rgba(15,23,42,.12);--rail:#e8edf3;
  --overlay-border:#c7d0dc;
  --z-extension-overlay:2147483647;--z-extension-video:2147483646;
}
`

/*
 * Why `--on-primary` is NOT white in the dark theme
 * --------------------------------------------------
 * The dark scheme's `--primary` (#5ea2f3) is a *light* blue, because it is
 * used far more often as text/icon ink on a near-black canvas (`.scan-button`,
 * `.video-more`, `.hlsd-badge`, `.section-title b`, focus rings, ...) than as a
 * fill. A light ink needs a dark surface: #5ea2f3 on --bg reads at 6.8:1.
 *
 * White text on that same light blue is only 2.65:1, and on the hover blue
 * (#7eb7f6) 2.10:1 -- both far below the 3.0:1 floor for UI text. That is the
 * same trap the desktop palette hit with `destructiveFill`: a token that is
 * correct as *ink* is wrong as a *fill*.
 *
 * So the fill keeps its light blue and the on-colour becomes the dark ink
 * (the canvas colour itself), which reads at 6.8:1 on --primary and 8.6:1 on
 * --primary-hover. The light scheme is unaffected: #2563eb is dark enough for
 * white text at 5.2:1, so `--on-primary` stays #ffffff there.
 *
 * Do not "restore" white here without re-checking every filled control:
 * .hlsd-button.primary, .download, .hover-action.primary, .video-download,
 * .update-notice button, .video-download b.
 *
 * The light theme's --faint was #64748b (slate-500) and only reached 4.23:1 on
 * --bg (#eef2f6), below AA for the section headers, the empty-state hint and
 * the disabled scan button that all use it. #5f6e83 is the *lightest* value
 * that clears 4.5:1 (4.62 on --bg, 5.19 on --surface), so the change stays as
 * small as the requirement allows.
 *
 * Why the `--*-ink` tokens exist
 * ------------------------------
 * The accent tokens above (`--primary`, `--green`, `--red`, `--purple`) are tuned
 * to be read *directly on the canvas*: they are the ink of section headers, icons,
 * links and focus rings. Several components, however, paint a low-alpha tint of an
 * accent and then put that same accent on top of it as text:
 *
 *   .hlsd-badge          --primary on mix(--primary 20%, --surface-2)
 *   .empty-icon          --primary on mix(--primary 10%, --surface)
 *   .scan-button:hover   --primary on mix(--primary 10%, --surface)
 *   .hlsd-button.active  --green   on mix(--green   16%, --surface-3)
 *   .result              --green   on mix(--green   14%, --surface)
 *   .pin.active          --green   on mix(--green   18%, --surface-3)
 *   .result.error        --red     on mix(--red     12%, --surface)
 *   .send-error          --red     on mix(--red     10%, --surface)
 *   .push-button         --purple  on mix(--purple  22%, --surface-3)
 *   .cast-button         --purple  on mix(--purple  22%, --surface-3)
 *
 * The tint lifts the background *towards* the accent, which eats exactly the
 * contrast the accent was relying on. Measured worst case was 3.41:1 (dark
 * .push-button) and 3.48:1 (light .hlsd-button.active) -- both below AA.
 *
 * The tint is what carries the "success / error / pinned" cue, so it stays. Only
 * the *label* moves to a dedicated ink that is the same hue, pushed away from the
 * tint: lighter in the dark scheme, darker in the light one. That is why each ink
 * is declared separately per theme rather than derived.
 *
 * Do not replace these with the plain accent, and do not collapse them into one
 * shared value -- each is the *minimum* shift that clears 4.5:1 across every
 * usage listed above (verified by the contrast guards in theme.test.ts, which
 * cover the hover states too).
 *
 * Hover states deliberately do NOT get an ink: they switch to the *solid* accent
 * with `--on-primary` (see .push-button:hover), which reads at 6.6:1 / 5.7:1 and
 * keeps the pill's hue stable. `.hlsd-button.active:hover` needs its own rule
 * because the generic `.hlsd-button:hover:not(:disabled)` is (0,3,0) and would
 * otherwise beat `.hlsd-button.active` (0,2,0), leaving green text on a blue tint.
 *
 * Why `--border` and `--control-border` are two different tokens
 * ---------------------------------------------------------------
 * `--border` is a *separator*: card edges, list dividers, table rules. It is
 * meant to be barely there, and 1.2-1.6:1 is the right answer for that job.
 *
 * A control boundary has a different job: it is what tells you "this is an
 * input, not a line of text". That falls under WCAG 1.4.11 (non-text contrast,
 * 3:1). Reusing the separator token there left form controls with a boundary of
 * 1.24-1.51:1 -- effectively invisible, so a <select> only read as a control
 * because of its text and the dropdown arrow.
 *
 * So `--control-border` is its own token, used ONLY by the interactive controls
 * (.hlsd-select, .quality-trigger, .empty-retry). Menus and popovers keep the
 * soft `--border` because elevation + shadow already separate them. Each value
 * is the *lightest* shift from `--border` that clears 3:1 against every surface
 * a control can sit on (--surface-2 / --surface / --surface-3 / --bg); the
 * binding constraint is --surface-3, so re-check that one before changing these.
 */

/** Base primitives shared by popup and in-page panels. */
export const THEME_BASE_CSS = `
.hlsd-button{display:inline-flex;align-items:center;justify-content:center;gap:6px;height:32px;padding:0 12px;border:1px solid transparent;border-radius:7px;background:var(--surface-3);color:var(--text);cursor:pointer;font:600 13px/1 system-ui,sans-serif;letter-spacing:0;white-space:nowrap;transition:background-color .18s ease,color .18s ease,border-color .18s ease,transform .12s ease}
.hlsd-button:hover:not(:disabled){background:color-mix(in srgb,var(--primary) 12%,var(--surface-3))}
.hlsd-button:active:not(:disabled){transform:scale(.975)}
.hlsd-button:disabled{opacity:.45;cursor:default}
.hlsd-button.primary{background:var(--primary);color:var(--on-primary)}
.hlsd-button.primary:hover:not(:disabled){background:var(--primary-hover)}
.hlsd-button.subtle{background:transparent;color:var(--muted)}
.hlsd-button.subtle:hover:not(:disabled){background:var(--surface-2);color:var(--text)}
.hlsd-button.active{background:color-mix(in srgb,var(--green) 16%,var(--surface-3));color:var(--green-ink);border-color:color-mix(in srgb,var(--green) 34%,transparent)}
.hlsd-button.active:hover:not(:disabled){background:color-mix(in srgb,var(--green) 22%,var(--surface-3))}
.hlsd-button:focus-visible{outline:2px solid var(--primary);outline-offset:1px}
.hlsd-icon{display:inline-flex;align-items:center;justify-content:center;width:15px;height:15px;flex:none}
.hlsd-icon svg{display:block;width:100%;height:100%;fill:none;stroke:currentColor;stroke-linecap:round;stroke-linejoin:round;stroke-width:1.8}
.hlsd-button.primary .hlsd-icon{width:14px;height:14px}
.hlsd-button.busy .hlsd-icon{animation:hlsd-spin .8s linear infinite}
@keyframes hlsd-spin{to{transform:rotate(360deg)}}
.hlsd-select{height:30px;border:1px solid var(--control-border);border-radius:7px;background:var(--surface-2);color:var(--text);padding:0 8px;font:12.5px system-ui,sans-serif}
.hlsd-badge{display:inline-grid;place-items:center;min-width:20px;height:20px;padding:0 6px;border-radius:10px;background:color-mix(in srgb,var(--primary) 20%,var(--surface-2));color:var(--primary-ink);font:700 11.5px system-ui,sans-serif}
`

export function resolveTheme(preference: ThemePreference, systemDark: boolean): ResolvedTheme {
  if (preference === 'dark' || preference === 'light') return preference
  return systemDark ? 'dark' : 'light'
}

export function normalizeThemePreference(value: unknown): ThemePreference {
  return value === 'dark' || value === 'light' ? value : 'auto'
}

/**
 * Stamp the resolved theme on a surface root and keep it in sync with the
 * system scheme while the preference is 'auto'. Returns a cleanup function.
 */
export function applyTheme(
  root: HTMLElement,
  preference: ThemePreference,
  matchMediaFn: (query: string) => MediaQueryList = query => window.matchMedia(query),
): () => void {
  const media = matchMediaFn('(prefers-color-scheme: dark)')
  const stamp = () => {
    root.setAttribute('data-hlsd-theme', resolveTheme(preference, media.matches))
  }
  stamp()
  if (preference !== 'auto') return () => {}
  media.addEventListener('change', stamp)
  return () => media.removeEventListener('change', stamp)
}
