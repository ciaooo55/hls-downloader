import type { MediaResource } from './resources'
import { formatBytes, formatDuration } from './format'

export interface OverlayPosition {
  x: number
  y: number
}

export interface OverlaySize {
  width: number
  height: number
}

export interface OverlayViewport {
  width: number
  height: number
}

export type OverlayRect = Pick<DOMRectReadOnly, 'x' | 'y' | 'left' | 'top' | 'right' | 'bottom' | 'width' | 'height'>

/** Follow the rendered tree: slotted media belongs to the slot's clipping containers. */
export function* mediaOverlayAncestors(element: HTMLElement): Generator<HTMLElement> {
  for (let node: HTMLElement | null = element; node;) {
    yield node
    const root = node.getRootNode() as ShadowRoot
    node = node.assignedSlot || node.parentElement || (root.host as HTMLElement | undefined) || null
  }
}

/** Axis-aligned visible player bounds, including scroll containers and shadow hosts. */
export function visibleMediaRect(element: HTMLElement, viewport: OverlayViewport): OverlayRect | null {
  const view = element.ownerDocument.defaultView
  if (!element.isConnected || !view || element.ownerDocument.visibilityState === 'hidden') return null
  const ownStyle = view.getComputedStyle(element)
  if (ownStyle.visibility === 'hidden' || ownStyle.visibility === 'collapse') return null
  const rect = element.getBoundingClientRect()
  let left = Math.max(0, rect.left), top = Math.max(0, rect.top)
  let right = Math.min(viewport.width, rect.right), bottom = Math.min(viewport.height, rect.bottom)
  for (const node of mediaOverlayAncestors(element)) {
    const style = node === element ? ownStyle : view.getComputedStyle(node)
    if (style.display === 'none' || style.contentVisibility === 'hidden' || Number(style.opacity) === 0) return null
    if (node !== element && style.display !== 'contents') {
      const clipsX = /^(hidden|clip|scroll|auto)$/.test(style.overflowX)
      const clipsY = /^(hidden|clip|scroll|auto)$/.test(style.overflowY)
      if (clipsX || clipsY) {
        const bounds = node.getBoundingClientRect()
        const scaleX = bounds.width / (node.offsetWidth || bounds.width || 1)
        const scaleY = bounds.height / (node.offsetHeight || bounds.height || 1)
        const x = bounds.left + node.clientLeft * scaleX
        const y = bounds.top + node.clientTop * scaleY
        if (clipsX) { left = Math.max(left, x); right = Math.min(right, x + node.clientWidth * scaleX) }
        if (clipsY) { top = Math.max(top, y); bottom = Math.min(bottom, y + node.clientHeight * scaleY) }
      }
    }
  }
  if (right <= left || bottom <= top) return null
  return { x: left, y: top, left, top, right, bottom, width: right - left, height: bottom - top }
}

/** Resolve paint order through player controls and shadow hosts, stopping at visible covers. */
export function mediaElementAtPoint<T extends HTMLElement>(elements: readonly T[], point: OverlayPosition): T | null {
  const document = elements[0]?.ownerDocument
  const view = document?.defaultView
  if (!document || !view || point.x < 0 || point.y < 0) return null
  const candidates = new Set<HTMLElement>(elements)
  const roots = new Map<Element, ShadowRoot>()
  for (const element of elements) {
    for (const ancestor of mediaOverlayAncestors(element)) {
      const root = ancestor.getRootNode() as ShadowRoot
      if (root.host) roots.set(root.host, root)
    }
  }
  const visited = new Set<Document | ShadowRoot>()
  function* hits(root: Document | ShadowRoot): Generator<Element> {
    if (visited.has(root)) return
    visited.add(root)
    for (const element of root.elementsFromPoint(point.x, point.y)) {
      if (root !== document && element.getRootNode() !== root && element.assignedSlot?.getRootNode() !== root) continue
      const shadow = element.shadowRoot || roots.get(element)
      if (shadow) yield* hits(shadow)
      yield element
    }
  }
  for (const element of hits(document)) {
    if (candidates.has(element as HTMLElement)) return element as T
    const style = view.getComputedStyle(element)
    if (style.visibility !== 'visible') continue
    if ([...mediaOverlayAncestors(element as HTMLElement)].some(ancestor => Number(view.getComputedStyle(ancestor).opacity) === 0)) continue
    const transparentBackground = style.backgroundColor === 'transparent'
      || /^rgba\([^)]*,\s*0(?:\.0+)?%?\s*\)$/.test(style.backgroundColor)
      || /\/\s*0(?:\.0+)?%?\s*\)$/.test(style.backgroundColor)
    if (!transparentBackground || style.backgroundImage !== 'none'
      || /^(VIDEO|IFRAME|IMG|CANVAS|OBJECT|EMBED)$/.test(element.tagName)) return null
  }
  return null
}

export type OverlayAction = 'download' | 'tvbox' | 'cast'

/** Pointer-intent grace period for moving from the compact control into its hover card. */
export const HOVER_DISMISS_DELAY_MS = 500

export function shouldKeepHoverOpen(pointerInside: boolean, focusInside: boolean): boolean {
  return pointerInside || focusInside
}

export interface OverlayResourceDetails {
  title: string
  facts: string[]
  source: string
  state: string
}

/** Display a useful source location without leaking credentials or signed query parameters. */
export function safeResourceLocation(value: string): string {
  try {
    const url = new URL(value)
    const segments = decodeURIComponent(url.pathname).split('/').filter(Boolean).slice(-2)
    const path = segments.length ? `/${segments.join('/')}` : ''
    return `${url.hostname}${path}`.slice(0, 110)
  } catch {
    return '媒体来源'
  }
}

/** Compact, user-facing metadata for the hover surface beside a playing video. */
export function overlayResourceDetails(resource: MediaResource): OverlayResourceDetails {
  let leaf = ''
  try { leaf = decodeURIComponent(new URL(resource.url).pathname.split('/').pop() || '') } catch {}
  const title = String(resource.title || resource.filename || leaf || '当前视频').trim()
  const protocol = resource.kind === 'hls' ? 'HLS'
    : resource.kind === 'dash' ? 'DASH'
      : resource.kind === 'media' ? '媒体文件'
        : resource.kind === 'magnet' ? '磁力链接' : '文件'
  const likelyBytes = Number(resource.size || resource.estimatedSize || 0)
  const facts = [
    protocol,
    resource.lowLatencyLive ? '低延迟直播' : resource.isLive === true ? '直播' : '',
    resource.quality || '',
    resource.width && resource.height ? `${resource.width}×${resource.height}` : '',
    resource.bandwidth ? `${(resource.bandwidth / 1_000_000).toFixed(1)} Mbps` : '',
    resource.isLive === true ? '' : formatDuration(Number(resource.duration || 0)),
    likelyBytes ? `${resource.size ? '' : '约 '}${formatBytes(likelyBytes)}` : '',
  ].filter(Boolean)
  return {
    title,
    facts: [...new Set(facts)].slice(0, 6),
    source: safeResourceLocation(resource.url),
    state: resource.inspected ? '清单已解析' : '已匹配当前播放',
  }
}

export function overlaySendKey(fingerprint: string, action: OverlayAction = 'download'): string {
  return action === 'download' ? fingerprint : `${fingerprint}:${action}`
}

export function overlayActionFallback(action: OverlayAction): string {
  if (action === 'tvbox') return 'TVBox'
  if (action === 'cast') return '投屏'
  return '下载'
}

/** Only show an actionable control after matching the player's actual source. */
export function shouldShowMediaOverlay(input: {
  hasPlayback: boolean
  hasActiveVideo: boolean
  resourceCount: number
}): boolean {
  return input.hasPlayback && input.hasActiveVideo && input.resourceCount > 0
}

/** Clamp an overlay to the visible viewport without persisting a cross-site coordinate. */
export function clampOverlayPosition(
  position: OverlayPosition,
  size: OverlaySize,
  viewport: OverlayViewport,
  margin = 10,
): OverlayPosition {
  const maxX = Math.max(margin, viewport.width - Math.max(0, size.width) - margin)
  const maxY = Math.max(margin, viewport.height - Math.max(0, size.height) - margin)
  return {
    x: Math.max(margin, Math.min(position.x, maxX)),
    y: Math.max(margin, Math.min(position.y, maxY)),
  }
}
