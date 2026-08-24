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
  --border:#383d43;--text:#f4f5f6;--muted:#c5c9cf;--faint:#969ca4;
  --primary:#5ea2f3;--primary-hover:#7eb7f6;--on-primary:#ffffff;
  --green:#22c55e;--amber:#f59e0b;--red:#ef4444;--purple:#a78bfa;
  --shadow:rgba(0,0,0,.35);--rail:#1c1f23;
  --overlay-border:#464c53;
  --z-extension-overlay:2147483647;--z-extension-video:2147483646;
}
[data-hlsd-theme="light"]{
  color-scheme:light;
  --bg:#eef2f6;--surface:#ffffff;--surface-2:#f5f7fa;--surface-3:#e8edf3;
  --border:#d8e0ea;--text:#0f172a;--muted:#475569;--faint:#64748b;
  --primary:#2563eb;--primary-hover:#1d4ed8;--on-primary:#ffffff;
  --green:#16a34a;--amber:#d97706;--red:#dc2626;--purple:#7c3aed;
  --shadow:rgba(15,23,42,.12);--rail:#e8edf3;
  --overlay-border:#c7d0dc;
  --z-extension-overlay:2147483647;--z-extension-video:2147483646;
}
`

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
.hlsd-button.active{background:color-mix(in srgb,var(--green) 16%,var(--surface-3));color:var(--green);border-color:color-mix(in srgb,var(--green) 34%,transparent)}
.hlsd-button:focus-visible{outline:2px solid var(--primary);outline-offset:1px}
.hlsd-icon{display:inline-flex;align-items:center;justify-content:center;width:15px;height:15px;flex:none}
.hlsd-icon svg{display:block;width:100%;height:100%;fill:none;stroke:currentColor;stroke-linecap:round;stroke-linejoin:round;stroke-width:1.8}
.hlsd-button.primary .hlsd-icon{width:14px;height:14px}
.hlsd-button.busy .hlsd-icon{animation:hlsd-spin .8s linear infinite}
@keyframes hlsd-spin{to{transform:rotate(360deg)}}
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
