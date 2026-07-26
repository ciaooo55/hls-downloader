/**
 * Shared design tokens for every extension surface (popup and in-page
 * shadow-DOM panels), aligned with the desktop app's Cockpit-style palette
 * (frontend/src/styles.css) so both products read as one family.
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
  --bg:#1a1815;--surface:#24211d;--surface-2:#2b2823;--surface-3:#332f29;
  --border:#3d3831;--text:#ece9e2;--muted:#a09c93;--faint:#75716a;
  --primary:#d97757;--primary-hover:#e28a6d;--on-primary:#1a1815;
  --green:#4fa980;--amber:#d2a04a;--red:#dd6f63;--purple:#a98bc9;
  --shadow:rgba(0,0,0,.45);
  --overlay-border:rgba(74,68,60,.9);
}
[data-hlsd-theme="light"]{
  color-scheme:light;
  --bg:#faf9f5;--surface:#fffefb;--surface-2:#f4f1e8;--surface-3:#f0ece1;
  --border:#ddd8ca;--text:#1f1e1c;--muted:#6b6a63;--faint:#91908a;
  --primary:#c15f3c;--primary-hover:#a94f2f;--on-primary:#fff;
  --green:#2f7a5b;--amber:#a1691c;--red:#b8453c;--purple:#7a5c9e;
  --shadow:rgba(31,30,28,.16);
  --overlay-border:rgba(221,216,202,.95);
}
`

/** Base primitives shared by popup and in-page panels. */
export const THEME_BASE_CSS = `
.hlsd-button{display:inline-flex;align-items:center;justify-content:center;gap:6px;height:32px;padding:0 12px;border:1px solid transparent;border-radius:7px;background:var(--surface-3);color:var(--text);cursor:pointer;font:600 13px/1 system-ui,sans-serif;letter-spacing:0;white-space:nowrap}
.hlsd-button:hover:not(:disabled){background:color-mix(in srgb,var(--primary) 12%,var(--surface-3))}
.hlsd-button:disabled{opacity:.45;cursor:default}
.hlsd-button.primary{background:var(--primary);color:var(--on-primary)}
.hlsd-button.primary:hover:not(:disabled){background:var(--primary-hover)}
.hlsd-button.subtle{background:transparent;color:var(--muted)}
.hlsd-button.subtle:hover:not(:disabled){background:var(--surface-2);color:var(--text)}
.hlsd-button.active{background:color-mix(in srgb,var(--green) 16%,var(--surface-3));color:var(--green);border-color:color-mix(in srgb,var(--green) 34%,transparent)}
.hlsd-button:focus-visible{outline:2px solid var(--primary);outline-offset:1px}
.hlsd-select{height:30px;border:1px solid var(--border);border-radius:7px;background:var(--surface-2);color:var(--text);padding:0 8px;font:12.5px system-ui,sans-serif}
.hlsd-badge{display:inline-grid;place-items:center;min-width:20px;height:20px;padding:0 6px;border-radius:10px;background:color-mix(in srgb,var(--primary) 20%,var(--surface-2));color:var(--primary);font:700 11.5px system-ui,sans-serif}
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
