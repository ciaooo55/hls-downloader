export type BrowserFamily = 'edge' | 'chrome' | 'chromium' | 'brave' | 'vivaldi' | 'opera' | 'firefox'

export const BROWSER_CLIENT_ID_STORAGE_KEY = 'desktopClientId'

interface LocalStorageLike {
  get(key: string): Promise<Record<string, unknown>>
  set(value: Record<string, unknown>): Promise<void>
}

export function detectBrowserFamily(runtimeUrl: string, userAgent = '', braveApiPresent = false): BrowserFamily {
  if (runtimeUrl.startsWith('moz-extension://')) return 'firefox'
  if (/\bEdg\//i.test(userAgent)) return 'edge'
  if (/\bOPR\//i.test(userAgent)) return 'opera'
  if (/\bVivaldi\//i.test(userAgent)) return 'vivaldi'
  if (braveApiPresent || /\bBrave\//i.test(userAgent)) return 'brave'
  if (/\b(?:Chrome|Chromium)\//i.test(userAgent)) return 'chrome'
  return 'chromium'
}

export async function stableBrowserClientId(
  storage: LocalStorageLike,
  createId: () => string,
): Promise<string> {
  const stored = await storage.get(BROWSER_CLIENT_ID_STORAGE_KEY)
  const current = String(stored[BROWSER_CLIENT_ID_STORAGE_KEY] || '').trim()
  if (current) return current
  const created = createId()
  await storage.set({ [BROWSER_CLIENT_ID_STORAGE_KEY]: created })
  return created
}
