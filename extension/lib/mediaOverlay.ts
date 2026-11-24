import { downloadSelectionKey, likelyResourceBytes, selectMediaVariant, type MediaResource } from './resources'
import { formatBitrate, formatBytes, formatDuration } from './format'

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

export interface OverlayQualityOption {
  /** Stable key for keyed DOM reconciliation and send-state lookup. */
  key: string
  /** Concrete resource/variant ready to pass directly to sendResource(). */
  resource: MediaResource
  /** Primary concise label, e.g. '1080p', '720p', 'MP4'. */
  label: string
  /** Secondary concise detail, e.g. '5.2 Mbps', '120 MB'. */
  detail: string
  /** Accessible button description without exposing signed URL parameters. */
  ariaLabel: string
  /** Highest-quality recommended option. */
  recommended: boolean
}

function overlayFormatTag(resource: Pick<MediaResource, 'kind' | 'url' | 'mimeType'>): string {
  if (resource.kind === 'hls') return 'HLS'
  if (resource.kind === 'dash') return 'DASH'
  if (resource.kind === 'magnet') return '磁力'
  try {
    const ext = new URL(resource.url).pathname.match(/\.([a-z0-9]{2,5})$/i)?.[1]?.toUpperCase()
    if (ext && ['MP4', 'M4V', 'WEBM', 'MKV', 'MOV', 'AVI', 'FLV', 'TS', 'M4A', 'MP3', 'AAC', 'FLAC', 'WAV', 'OGG', 'OPUS'].includes(ext)) {
      return ext
    }
  } catch {}
  const mime = (resource.mimeType || '').toLowerCase()
  if (mime.includes('mp4')) return 'MP4'
  if (mime.includes('webm')) return 'WEBM'
  if (mime.startsWith('audio/')) return '音频'
  return resource.kind === 'media' ? '视频' : '文件'
}

function cleanQualityBadge(quality?: string, height?: number): string {
  const normalized = String(quality || '').replace(/^最高\s*/, '').trim()
  if (normalized) return normalized
  return height && height > 0 ? `${height}p` : ''
}

/**
 * Build up to `limit` 1-click quality pill options sorted highest quality first.
 *
 * Returns an empty list when only a single stream/quality exists so single-item
 * players stay compact without a redundant selector list.
 */
export function overlayQualityOptions(choices: readonly MediaResource[], limit = 6): OverlayQualityOption[] {
  interface Candidate {
    key: string
    resource: MediaResource
    formatTag: string
    label: string
    width: number
    height: number
    bandwidth: number
    exactSize: number
    estimatedSize: number
  }

  const raw: Candidate[] = []
  for (const choice of choices) {
    const formatTag = overlayFormatTag(choice)
    if (choice.variants?.length) {
      for (const variant of choice.variants) {
        const selected = selectMediaVariant(choice, variant)
        const height = Number(variant.height || 0)
        const width = Number(variant.width || 0)
        const bandwidth = Number(variant.bandwidth || 0)
        const estimated = choice.isLive === true
          ? 0
          : (Number(choice.duration) > 0 && bandwidth > 0
              ? Math.round(Number(choice.duration) * bandwidth / 8)
              : 0)
        const label = cleanQualityBadge(variant.quality, height) || formatTag
        raw.push({
          key: downloadSelectionKey(selected),
          resource: selected,
          formatTag,
          label,
          width,
          height,
          bandwidth,
          exactSize: 0,
          estimatedSize: estimated,
        })
      }
    } else {
      const height = Number(choice.height || 0)
      const width = Number(choice.width || 0)
      const bandwidth = Number(choice.bandwidth || 0)
      const exactSize = Number(choice.size || 0)
      const estimatedSize = exactSize > 0
        ? 0
        : (choice.isLive === true ? 0 : likelyResourceBytes({ ...choice, size: 0 }))
      const label = cleanQualityBadge(choice.quality, height) || formatTag
      raw.push({
        key: downloadSelectionKey(choice),
        resource: choice,
        formatTag,
        label,
        width,
        height,
        bandwidth,
        exactSize,
        estimatedSize,
      })
    }
  }

  raw.sort((left, right) => right.height - left.height
    || right.bandwidth - left.bandwidth
    || (right.exactSize || right.estimatedSize) - (left.exactSize || left.estimatedSize)
    || right.width - left.width)

  const deduped: Candidate[] = []
  const seen = new Set<string>()
  for (const candidate of raw) {
    if (seen.has(candidate.key)) continue
    seen.add(candidate.key)
    deduped.push(candidate)
    if (deduped.length >= Math.max(0, limit)) break
  }

  if (deduped.length < 2) return []

  const multipleFormats = new Set(deduped.map(entry => entry.formatTag)).size > 1
  return deduped.map((entry, index) => {
    const parts: string[] = []
    if (multipleFormats && entry.label !== entry.formatTag) parts.push(entry.formatTag)
    if (entry.bandwidth > 0) parts.push(formatBitrate(entry.bandwidth))
    if (entry.exactSize > 0) parts.push(formatBytes(entry.exactSize))
    else if (entry.estimatedSize > 0) parts.push(`约 ${formatBytes(entry.estimatedSize)}`)
    if (!parts.length && entry.width > 0 && entry.height > 0 && !entry.label.includes(`${entry.height}p`)) {
      parts.push(`${entry.width}×${entry.height}`)
    }
    if (!parts.length && entry.label !== entry.formatTag) parts.push(entry.formatTag)
    const detail = parts.join(' · ')
    return {
      key: entry.key,
      resource: entry.resource,
      label: entry.label,
      detail,
      ariaLabel: `下载 ${[entry.label, detail].filter(Boolean).join(' ')}`,
      recommended: index === 0,
    }
  })
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
    resource.bandwidth ? formatBitrate(resource.bandwidth) : '',
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
