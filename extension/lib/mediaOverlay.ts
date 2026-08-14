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

export function overlaySendKey(fingerprint: string, action: OverlayAction = 'download'): string {
  return action === 'download' ? fingerprint : `${fingerprint}:${action}`
}

export function overlayActionFallback(action: OverlayAction): string {
  if (action === 'tvbox') return '推送链接'
  if (action === 'cast') return '投屏链接'
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
