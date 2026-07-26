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
  --bg:#17191d;--surface:#202328;--surface-2:#272b31;--surface-3:#30353c;
  --border:#383d45;--text:#e3e7ec;--muted:#97a0ab;--faint:#68727e;
  --primary:#2583c5;--primary-hover:#3092d6;
  --green:#39a875;--amber:#d69b3a;--red:#dc5c5c;--purple:#9c72d2;
  --shadow:rgba(0,0,0,.35);
  --overlay-border:rgba(56,61,69,.9);
}
[data-hlsd-theme="light"]{
  color-scheme:light;
  --bg:#edf0f3;--surface:#fff;--surface-2:#f5f7f9;--surface-3:#e8edf1;
  --border:#cfd6dd;--text:#202831;--muted:#5d6874;--faint:#7d8791;
  --primary:#126fae;--primary-hover:#0f6098;
  --green:#267957;--amber:#93671c;--red:#bd4640;--purple:#74509e;
  --shadow:rgba(31,42,54,.18);
  --overlay-border:rgba(207,214,221,.95);
}
`

/** Base primitives shared by popup and in-page panels. */
export const THEME_BASE_CSS = `
.hlsd-button{display:inline-flex;align-items:center;justify-content:center;gap:5px;height:28px;padding:0 10px;border:1px solid transparent;border-radius:6px;background:var(--surface-3);color:var(--text);cursor:pointer;font:600 11px/1 system-ui,sans-serif;letter-spacing:0}
.hlsd-button:hover:not(:disabled){background:color-mix(in srgb,var(--primary) 12%,var(--surface-3))}
.hlsd-button:disabled{opacity:.45;cursor:default}
.hlsd-button.primary{background:var(--primary);color:#fff}
.hlsd-button.primary:hover:not(:disabled){background:var(--primary-hover)}
.hlsd-button.subtle{background:transparent;color:var(--muted)}
.hlsd-button.subtle:hover:not(:disabled){background:var(--surface-2);color:var(--text)}
.hlsd-button.active{background:color-mix(in srgb,var(--green) 16%,var(--surface-3));color:var(--green)}
.hlsd-button:focus-visible{outline:2px solid var(--primary);outline-offset:1px}
.hlsd-select{height:26px;border:1px solid var(--border);border-radius:5px;background:var(--surface-2);color:var(--text);padding:0 6px;font:10.5px system-ui,sans-serif}
.hlsd-badge{display:inline-grid;place-items:center;min-width:18px;height:17px;padding:0 5px;border-radius:9px;background:color-mix(in srgb,var(--primary) 18%,var(--surface-2));color:var(--primary);font:700 10px system-ui,sans-serif}
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
