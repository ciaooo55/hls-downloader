import { type MediaResource } from './resources'
import { ordinaryFileResponseIdentified } from './fileTakeover'

export type ObservedDownloadResource = Pick<MediaResource,
  'url' | 'kind' | 'mimeType' | 'size' | 'filename' | 'pageUrl' | 'tabId' | 'frameId' | 'statusCode' | 'method' | 'requestHeaders'>

export interface DirectResponseDetails {
  type?: string
  method?: string
  statusCode?: number
}

export interface ObservedDirectResponse {
  disposition: string
  resource: ObservedDownloadResource | null
}

function hasDownloadHeaders(disposition: string, resource: ObservedDownloadResource | null): boolean {
  if (!resource) return false
  return /(?:^|;)\s*attachment(?:;|$)/i.test(disposition)
    || Boolean(resource.filename)
    || resource.mimeType?.toLowerCase().includes('application/octet-stream') === true
}

/**
 * Decide whether response headers are strong enough to offer a browser
 * navigation before Chrome creates its DownloadItem.  XHR/fetch responses
 * deliberately return false and continue through downloads.onCreated, where
 * click intent and the final browser item are available.
 */
export function isEarlyDirectDownloadResponse(
  details: DirectResponseDetails,
  observed: ObservedDirectResponse,
): boolean {
  if (!['main_frame', 'sub_frame'].includes(String(details.type || ''))) return false
  if (String(details.method || 'GET').toUpperCase() !== 'GET') return false
  if (Number(details.statusCode) < 200 || Number(details.statusCode) >= 300) return false
  if (hasDownloadHeaders(observed.disposition, observed.resource)) return true
  const resource = observed.resource
  if (!resource) return false
  // Some CDNs serve direct MP4/WebM responses without Content-Disposition.
  // Restrict the fallback to already-classified media/file responses so an
  // ordinary HTML, script, or JSON navigation cannot be pre-offered.
  if (resource.kind === 'media' && /^(?:video|audio)\//i.test(resource.mimeType || '')) return true
  return ordinaryFileResponseIdentified(resource)
}
