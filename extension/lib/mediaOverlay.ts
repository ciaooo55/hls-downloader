import type { MediaResource } from './resources'

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

export type OverlayAction = 'download' | 'tvbox' | 'cast'

export interface OverlayResourceDetails {
  title: string
  facts: string[]
  source: string
  state: string
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return ''
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value >= 100 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`
}

function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return ''
  const total = Math.round(seconds)
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor(total % 3600 / 60)
  const remainder = total % 60
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${String(remainder).padStart(2, '0')}`
    : `${minutes}:${String(remainder).padStart(2, '0')}`
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

/**
 * A visible playing video is enough to show a non-actionable identifying
 * state. The download action is enabled separately only after resource
 * association succeeds.
 */
export function shouldShowMediaOverlay(input: {
  hasPlayback: boolean
  hasActiveVideo: boolean
  resourceCount: number
}): boolean {
  return input.hasPlayback && input.hasActiveVideo
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
