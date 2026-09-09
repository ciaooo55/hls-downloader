import { classifyResource } from './resources'

export interface ContextMenuTarget {
  linkUrl?: string
  srcUrl?: string
  mediaType?: string
}

export interface ContextMenuCapabilities {
  url: string
  download: boolean
  media: boolean
}

export function contextMenuCapabilities(target: ContextMenuTarget): ContextMenuCapabilities {
  const url = String(target.srcUrl || target.linkUrl || '')
  if (/^blob:/i.test(url)) {
    // A blob URL is owned by the page process and cannot be replayed by the
    // desktop downloader or sent to a LAN receiver. The click handler can
    // still open the correlated media panel instead of pretending the blob is
    // a direct download/cast target.
    return { url, download: false, media: false }
  }
  const mediaContext = target.mediaType === 'video' || target.mediaType === 'audio'
  const kind = classifyResource(url)
  const media = mediaContext || kind === 'hls' || kind === 'dash' || kind === 'media'
  const download = mediaContext || /^(?:https?|magnet):/i.test(url)
  return { url, download, media }
}
