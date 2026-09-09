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
  const mediaContext = target.mediaType === 'video' || target.mediaType === 'audio'
  const kind = classifyResource(url)
  const media = mediaContext || kind === 'hls' || kind === 'dash' || kind === 'media'
  const download = mediaContext || /^(?:https?|magnet):/i.test(url)
  return { url, download, media }
}
