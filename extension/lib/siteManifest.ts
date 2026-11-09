import { manifestMimeType } from './manifestSniff'

export const SITE_RESPONSE_LIMIT = 2 * 1024 * 1024

function youtubeHost(host: string): boolean {
  return host === 'youtube.com' || host.endsWith('.youtube.com')
}

/** A logical page identity, never a substitute for exact request/byte ownership. */
export function youtubePageVideoId(pageUrl: string): string {
  try {
    const page = new URL(pageUrl)
    if (page.protocol !== 'https:' || !(youtubeHost(page.hostname)
      || page.hostname === 'www.youtube-nocookie.com')) return ''
    const id = page.pathname === '/watch'
      ? page.searchParams.get('v') || ''
      : /^\/(?:embed|shorts|live)\/([^/]+)\/?$/.exec(page.pathname)?.[1] || ''
    return /^[\w-]{1,64}$/.test(id) ? id : ''
  } catch { return '' }
}

/** Only the player metadata endpoint opts in; ordinary JSON stays untouched. */
export function shouldInspectSiteResponse(url: string, pageUrl: string, mimeType = ''): boolean {
  if (!youtubePageVideoId(pageUrl)) return false
  try {
    const request = new URL(url)
    const mime = mimeType.split(';', 1)[0].trim().toLowerCase()
    return request.protocol === 'https:' && youtubeHost(request.hostname)
      && request.pathname === '/youtubei/v1/player'
      && (!mime || mime === 'application/json' || mime === 'text/plain')
  } catch { return false }
}

export function siteManifestCandidates(url: string, pageUrl: string, body: unknown): Array<{
  url: string
  mimeType: string
  videoId: string
}> {
  if (!shouldInspectSiteResponse(url, pageUrl)) return []
  if (typeof body === 'string') {
    if (body.length > SITE_RESPONSE_LIMIT) return []
    try { body = JSON.parse(body.replace(/^\)\]\}'\s*\n/, '')) } catch { return [] }
  }
  if (!body || typeof body !== 'object') return []
  const player = body as { videoDetails?: { videoId?: unknown }; streamingData?: Record<string, unknown> }
  const videoId = youtubePageVideoId(pageUrl)
  if (player.videoDetails?.videoId !== videoId) return []
  const candidates = []
  for (const [field, kind] of [['hlsManifestUrl', 'hls'], ['dashManifestUrl', 'dash']] as const) {
    const value = player.streamingData?.[field]
    if (typeof value !== 'string' || value.length > 16 * 1024) continue
    try {
      const manifest = new URL(value)
      const host = manifest.hostname
      if (manifest.protocol !== 'https:' || manifest.username || manifest.password
        || !(youtubeHost(host) || host === 'googlevideo.com' || host.endsWith('.googlevideo.com'))
        || !(kind === 'hls'
          ? /(?:\.m3u8?$|^\/api\/manifest\/hls(?:_variant)?\/)/i.test(manifest.pathname)
          : /(?:\.mpd$|^\/api\/manifest\/dash\/)/i.test(manifest.pathname))) continue
      candidates.push({ url: value, mimeType: manifestMimeType(kind), videoId })
    } catch {}
  }
  return candidates
}
