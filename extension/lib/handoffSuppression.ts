import type { ResourceKind } from './resources'

export const HANDOFF_SUPPRESSION_STORAGE_KEY = 'autoHandoffSuppressions'

export interface HandoffSuppression {
  host: string
  kind: ResourceKind
}

const RESOURCE_KINDS = new Set<ResourceKind>(['hls', 'dash', 'media', 'file', 'magnet'])

function hostForPage(value: string): string {
  try {
    return new URL(value).hostname.toLowerCase()
  } catch {
    return ''
  }
}

export function normalizeHandoffSuppressions(value: unknown, limit = 100): HandoffSuppression[] {
  if (!Array.isArray(value)) return []
  const unique = new Map<string, HandoffSuppression>()
  for (const item of value) {
    if (!item || typeof item !== 'object') continue
    const raw = item as { host?: unknown, kind?: unknown }
    const host = String(raw.host || '').trim().toLowerCase().replace(/^\.+|\.+$/g, '')
    const kind = String(raw.kind || '').trim().toLowerCase() as ResourceKind
    if (!host || !RESOURCE_KINDS.has(kind)) continue
    unique.set(`${host}:${kind}`, { host, kind })
    if (unique.size >= limit) break
  }
  return [...unique.values()]
}

/** Match only the address-bar site, never a media/CDN hostname. */
export function isHandoffSuppressed(
  rules: HandoffSuppression[],
  pageUrl: string,
  kind: ResourceKind,
): boolean {
  const host = hostForPage(pageUrl)
  return Boolean(host && rules.some(rule => rule.host === host && rule.kind === kind))
}
