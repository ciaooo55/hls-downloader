import { manifestMimeType } from './manifestSniff'
import type { MediaAudioTrack, MediaResource, MediaVariant } from './resources'

export const SITE_RESPONSE_LIMIT = 2 * 1024 * 1024

export interface SiteManifestCandidate {
  url: string
  mimeType: string
  videoId: string
  inspection?: Partial<MediaResource>
}

function youtubeHost(host: string): boolean {
  return host === 'youtube.com' || host.endsWith('.youtube.com')
}

function bilibiliHost(host: string): boolean {
  return host === 'bilibili.com' || host.endsWith('.bilibili.com')
}

function bilibiliMediaHost(host: string): boolean {
  const normalized = host.toLowerCase()
  return normalized === 'bilivideo.com' || normalized.endsWith('.bilivideo.com')
    || normalized === 'bilivideo.cn' || normalized.endsWith('.bilivideo.cn')
    || normalized === 'bilivideo.net' || normalized.endsWith('.bilivideo.net')
    || normalized === 'hdslb.com' || normalized.endsWith('.hdslb.com')
    || normalized === 'akamaized.net' || normalized.endsWith('.akamaized.net')
    || normalized === 'biliapi.net' || normalized.endsWith('.biliapi.net')
    || normalized === 'acgvideo.com' || normalized.endsWith('.acgvideo.com')
    || normalized === 'szbdyd.com' || normalized.endsWith('.szbdyd.com')
}

const BILIBILI_PLAYURL_PATHS = new Set([
  '/x/player/wbi/playurl',
  '/x/player/playurl',
  '/pgc/player/web/playurl',
  '/pgc/player/web/v2/playurl',
])

const BILIBILI_AUDIO_TRACK_ID = /-(?:30216|30232|30280|30250|30251)\.m4s(?:$|[?#])/i

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

/** Logical video identity for Bilibili watch pages (including `/en/video/...`). */
export function bilibiliPageVideoId(pageUrl: string): string {
  try {
    const page = new URL(pageUrl)
    if (page.protocol !== 'https:' || !bilibiliHost(page.hostname)) return ''
    const videoMatch = /^\/(?:[a-z]{2}\/)?video\/(BV[A-Za-z0-9]+|av\d+)(?:\/|$)/i.exec(page.pathname)
    if (videoMatch?.[1]) return videoMatch[1]
    const bangumiMatch = /^\/(?:[a-z]{2}\/)?bangumi\/play\/((?:ep|ss)\d+)(?:\/|$)/i.exec(page.pathname)
    if (bangumiMatch?.[1]) return bangumiMatch[1]
    if (/^\/(?:[a-z]{2}\/)?list\//i.test(page.pathname)) {
      const bvid = page.searchParams.get('bvid') || ''
      if (/^BV[A-Za-z0-9]{4,32}$/i.test(bvid)) return bvid
    }
    return ''
  } catch { return '' }
}

export function sitePageVideoId(pageUrl: string): string {
  return youtubePageVideoId(pageUrl) || bilibiliPageVideoId(pageUrl)
}

/** True for Bilibili CDN full-track byte-ranged video `.m4s` streams appended to MSE. */
export function isBilibiliFullTrackMseUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) return false
    if (!bilibiliMediaHost(parsed.hostname)) return false
    if (!/\.m4s$/i.test(parsed.pathname)) return false
    return !BILIBILI_AUDIO_TRACK_ID.test(parsed.pathname)
  } catch {
    return false
  }
}

/** Only the player metadata endpoint opts in; ordinary JSON stays untouched. */
export function shouldInspectSiteResponse(url: string, pageUrl: string, mimeType = ''): boolean {
  try {
    const request = new URL(url)
    const mime = mimeType.split(';', 1)[0].trim().toLowerCase()
    if (mime && mime !== 'application/json' && mime !== 'text/plain') return false
    if (youtubePageVideoId(pageUrl)) {
      return request.protocol === 'https:'
        && (youtubeHost(request.hostname) || request.hostname === 'www.youtube-nocookie.com')
        && request.pathname === '/youtubei/v1/player'
    }
    if (bilibiliPageVideoId(pageUrl)) {
      return request.protocol === 'https:'
        && request.hostname === 'api.bilibili.com'
        && BILIBILI_PLAYURL_PATHS.has(request.pathname)
    }
    return false
  } catch { return false }
}

function validBilibiliStreamUrl(value: unknown): string {
  if (typeof value !== 'string' || !value.trim() || value.length > 16 * 1024) return ''
  try {
    const parsed = new URL(value)
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) return ''
    return bilibiliMediaHost(parsed.hostname) ? parsed.href : ''
  } catch {
    return ''
  }
}

function bilibiliTrackUrls(track: Record<string, unknown>): string[] {
  const candidates = [
    track.baseUrl,
    track.base_url,
    track.url,
    ...(Array.isArray(track.backupUrl) ? track.backupUrl : []),
    ...(Array.isArray(track.backup_url) ? track.backup_url : []),
  ]
  return [...new Set(candidates.map(validBilibiliStreamUrl).filter(Boolean))]
}

function codecPreference(codecs: string): number {
  const normalized = codecs.toLowerCase()
  if (normalized.startsWith('avc')) return 3
  if (normalized.startsWith('hev') || normalized.startsWith('hvc')) return 2
  if (normalized.startsWith('av01')) return 1
  return 0
}

/** Extract Bilibili DASH/durl streams from `window.__playinfo__` or `playurl` JSON. */
export function bilibiliPlayinfoCandidates(pageUrl: string, body: unknown): SiteManifestCandidate[] {
  const videoId = bilibiliPageVideoId(pageUrl)
  if (!videoId) return []
  if (typeof body === 'string') {
    if (body.length > SITE_RESPONSE_LIMIT) return []
    try { body = JSON.parse(body) } catch { return [] }
  }
  if (!body || typeof body !== 'object' || Array.isArray(body)) return []
  const root = body as Record<string, unknown>
  const resultObj = root.result && typeof root.result === 'object' ? root.result as Record<string, unknown> : null
  const payload = (root.data && typeof root.data === 'object' ? root.data : null)
    || (resultObj?.video_info && typeof resultObj.video_info === 'object' ? resultObj.video_info : null)
    || resultObj
    || root
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return []
  const data = payload as Record<string, unknown>
  const dash = data.dash && typeof data.dash === 'object' && !Array.isArray(data.dash)
    ? data.dash as Record<string, unknown>
    : null
  const durationSeconds = (() => {
    const dashDuration = Number(dash?.duration || 0)
    if (Number.isFinite(dashDuration) && dashDuration > 0) return Math.round(dashDuration)
    const timeMs = Number(data.timelength || 0)
    if (Number.isFinite(timeMs) && timeMs > 0) return Math.round(timeMs / 1000)
    return undefined
  })()

  if (dash && Array.isArray(dash.video) && dash.video.length > 0) {
    const playbackUrls = new Set<string>()
    type ParsedVideo = {
      url: string
      width: number
      height: number
      bandwidth: number
      quality: string
      codecs: string
      variantId: string
    }
    const parsedVideos: ParsedVideo[] = []
    for (const raw of dash.video) {
      if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue
      const track = raw as Record<string, unknown>
      const urls = bilibiliTrackUrls(track)
      if (!urls.length) continue
      urls.forEach(u => playbackUrls.add(u))
      const width = Math.max(0, Math.floor(Number(track.width || 0)))
      const height = Math.max(0, Math.floor(Number(track.height || 0)))
      const bandwidth = Math.max(0, Math.floor(Number(track.bandwidth || 0)))
      const codecs = String(track.codecs || '')
      const id = String(track.id || '')
      const quality = height > 0 ? `${height}p` : (id ? `Q${id}` : '')
      parsedVideos.push({
        url: urls[0],
        width,
        height,
        bandwidth,
        quality,
        codecs,
        variantId: `${id || height || 0}:${codecs || bandwidth || 0}`,
      })
    }
    const audioTracks: MediaAudioTrack[] = []
    if (Array.isArray(dash.audio)) {
      for (const raw of dash.audio) {
        if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue
        const track = raw as Record<string, unknown>
        const urls = bilibiliTrackUrls(track)
        if (!urls.length) continue
        urls.forEach(u => playbackUrls.add(u))
        audioTracks.push({
          label: String(track.codecs || track.id || '音频'),
          language: '',
          url: urls[0],
          default: audioTracks.length === 0,
        })
      }
    }
    if (!parsedVideos.length) return []
    parsedVideos.sort((left, right) => right.height - left.height
      || codecPreference(right.codecs) - codecPreference(left.codecs)
      || right.bandwidth - left.bandwidth)
    // Keep one preferred codec per resolution so the hover card shows clean quality choices.
    const byQuality = new Map<string, ParsedVideo>()
    for (const entry of parsedVideos) {
      const key = entry.height > 0 ? `${entry.height}p` : entry.variantId
      if (!byQuality.has(key)) byQuality.set(key, entry)
    }
    const chosen = [...byQuality.values()].slice(0, 8)
    const primary = chosen[0]
    const variants: MediaVariant[] = chosen.length > 1
      ? chosen.map(entry => ({
          url: entry.url,
          variantId: entry.variantId,
          ...(entry.width > 0 ? { width: entry.width } : {}),
          ...(entry.height > 0 ? { height: entry.height } : {}),
          ...(entry.bandwidth > 0 ? { bandwidth: entry.bandwidth } : {}),
          ...(entry.quality ? { quality: entry.quality } : {}),
        }))
      : []
    const estimatedSize = durationSeconds && primary.bandwidth > 0
      ? Math.round(durationSeconds * primary.bandwidth / 8)
      : undefined
    return [{
      url: primary.url,
      mimeType: 'video/mp4',
      videoId,
      inspection: {
        inspected: true,
        isLive: false,
        ...(durationSeconds ? { duration: durationSeconds } : {}),
        ...(primary.width > 0 ? { width: primary.width } : {}),
        ...(primary.height > 0 ? { height: primary.height } : {}),
        ...(primary.bandwidth > 0 ? { bandwidth: primary.bandwidth } : {}),
        ...(estimatedSize ? { estimatedSize } : {}),
        ...(primary.quality ? { quality: primary.quality } : {}),
        ...(variants.length ? { variants } : {}),
        ...(audioTracks.length ? { audioTracks } : {}),
        playbackUrls: [...playbackUrls].slice(0, 48),
      },
    }]
  }

  if (Array.isArray(data.durl) && data.durl.length > 0) {
    const first = data.durl[0]
    if (!first || typeof first !== 'object' || Array.isArray(first)) return []
    const urls = bilibiliTrackUrls(first as Record<string, unknown>)
    if (!urls.length) return []
    const size = Math.max(0, Math.floor(Number((first as Record<string, unknown>).size || 0)))
    const itemLengthMs = Number((first as Record<string, unknown>).length || 0)
    const duration = durationSeconds || (Number.isFinite(itemLengthMs) && itemLengthMs > 0 ? Math.round(itemLengthMs / 1000) : undefined)
    return [{
      url: urls[0],
      mimeType: 'video/mp4',
      videoId,
      inspection: {
        inspected: true,
        isLive: false,
        ...(duration ? { duration } : {}),
        ...(size > 0 ? { estimatedSize: size } : {}),
        playbackUrls: urls.slice(0, 48),
      },
    }]
  }

  return []
}

export function siteManifestCandidates(url: string, pageUrl: string, body: unknown): SiteManifestCandidate[] {
  if (!shouldInspectSiteResponse(url, pageUrl)) return []
  if (bilibiliPageVideoId(pageUrl)) {
    return bilibiliPlayinfoCandidates(pageUrl, body)
  }
  if (typeof body === 'string') {
    if (body.length > SITE_RESPONSE_LIMIT) return []
    try { body = JSON.parse(body.replace(/^\)\]\}'\s*\n/, '')) } catch { return [] }
  }
  if (!body || typeof body !== 'object') return []
  const player = body as { videoDetails?: { videoId?: unknown }; streamingData?: Record<string, unknown> }
  const videoId = youtubePageVideoId(pageUrl)
  if (player.videoDetails?.videoId !== videoId) return []
  const candidates: SiteManifestCandidate[] = []
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
