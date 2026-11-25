import { manifestMimeType } from './manifestSniff'
import type { MediaAudioTrack, MediaResource, MediaVariant } from './resources'

export const SITE_RESPONSE_LIMIT = 2 * 1024 * 1024

export interface SiteManifestCandidate {
  url: string
  mimeType: string
  videoId: string
  audioUrl?: string
  inspection?: Partial<MediaResource>
}

function youtubeHost(host: string): boolean {
  return host === 'youtube.com' || host.endsWith('.youtube.com')
}

function bilibiliHost(host: string): boolean {
  return host === 'bilibili.com' || host.endsWith('.bilibili.com')
}

function bilibiliDedicatedMediaHost(host: string): boolean {
  const normalized = host.toLowerCase()
  return normalized === 'bilivideo.com' || normalized.endsWith('.bilivideo.com')
    || normalized === 'bilivideo.cn' || normalized.endsWith('.bilivideo.cn')
    || normalized === 'bilivideo.net' || normalized.endsWith('.bilivideo.net')
    || normalized === 'hdslb.com' || normalized.endsWith('.hdslb.com')
    || normalized === 'biliapi.net' || normalized.endsWith('.biliapi.net')
    || normalized === 'acgvideo.com' || normalized.endsWith('.acgvideo.com')
    || normalized === 'szbdyd.com' || normalized.endsWith('.szbdyd.com')
}

function akamaizedHost(host: string): boolean {
  const normalized = host.toLowerCase()
  return normalized === 'akamaized.net' || normalized.endsWith('.akamaized.net')
}

function bilibiliAkamaiMseUrl(parsed: URL): boolean {
  const host = parsed.hostname.toLowerCase()
  const pathAndQuery = `${parsed.pathname}${parsed.search}`.toLowerCase()
  if (pathAndQuery.includes('/upgcxcode/') || pathAndQuery.includes('upos-')) return true
  if (/(?:^|[/_?&=-])bili(?:bilibili|video|api|cdn|[/_?&=-]|$)/i.test(pathAndQuery)) return true
  return /(?:^|\.)upos-/i.test(host) && /(?:^|\/)\d+-\d+-\d+\.m4s$/i.test(parsed.pathname)
}

function bilibiliMediaHost(host: string): boolean {
  return bilibiliDedicatedMediaHost(host) || akamaizedHost(host)
}

function bilibiliMseTrackHost(parsed: URL): boolean {
  if (bilibiliDedicatedMediaHost(parsed.hostname)) return true
  if (akamaizedHost(parsed.hostname)) return bilibiliAkamaiMseUrl(parsed)
  return false
}

const BILIBILI_PLAYURL_PATHS = new Set([
  '/x/player/wbi/playurl',
  '/x/player/playurl',
  '/pgc/player/web/playurl',
  '/pgc/player/web/v2/playurl',
  '/ogv/player/playview',
  '/pugv/player/web/playurl',
])

const BILIBILI_AUDIO_TRACK_ID = /-(?:30216|30232|30280|30250|30251|30255|30257|30259|30260)\.m4s(?:$|[?#])/i

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
    const bangumiMatch = /^\/(?:[a-z]{2}\/)?(?:bangumi|cheese)\/play\/((?:ep|ss)\d+)(?:\/|$)/i.exec(page.pathname)
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

/**
 * True when two URLs represent the same non-empty video playback target across
 * SPA `replaceState`/`pushState` normalizations (such as Bilibili `/video/BV...` ->
 * `/en/video/BV...` or tracking query stripping) while distinguishing multi-part
 * episodes (`?p=2`) and playlist switches (`?list=...`).
 */
export function sameSiteVideoIdentity(previousUrl: string, nextUrl: string): boolean {
  if (!previousUrl || !nextUrl) return false
  const prevId = sitePageVideoId(previousUrl)
  const nextId = sitePageVideoId(nextUrl)
  if (!prevId || prevId !== nextId) return false
  try {
    const prev = new URL(previousUrl)
    const next = new URL(nextUrl)
    if (prev.origin !== next.origin) return false
    if (bilibiliHost(prev.hostname)) {
      const prevPart = prev.searchParams.get('p') || '1'
      const nextPart = next.searchParams.get('p') || '1'
      return prevPart === nextPart
    }
    if (youtubeHost(prev.hostname) || prev.hostname === 'www.youtube-nocookie.com') {
      const prevList = prev.searchParams.get('list') || ''
      const nextList = next.searchParams.get('list') || ''
      const prevIndex = prev.searchParams.get('index') || ''
      const nextIndex = next.searchParams.get('index') || ''
      return prevList === nextList && prevIndex === nextIndex
    }
    return true
  } catch {
    return false
  }
}

/** True for Bilibili CDN full-track byte-ranged video `.m4s` streams appended to MSE. */
export function isBilibiliFullTrackMseUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) return false
    if (!bilibiliMseTrackHost(parsed)) return false
    if (!/\.m4s$/i.test(parsed.pathname)) return false
    return !BILIBILI_AUDIO_TRACK_ID.test(parsed.pathname)
  } catch {
    return false
  }
}

/** True for Bilibili CDN full-track byte-ranged audio `.m4s` streams appended to MSE. */
export function isBilibiliAudioTrackMseUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) return false
    if (!bilibiliMseTrackHost(parsed)) return false
    return BILIBILI_AUDIO_TRACK_ID.test(parsed.pathname)
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

function parseFrameRate(raw: unknown): number {
  if (typeof raw === 'number' && Number.isFinite(raw)) return raw
  const text = String(raw || '').trim()
  if (!text) return 0
  if (text.includes('/')) {
    const [num, den] = text.split('/', 2).map(Number)
    if (Number.isFinite(num) && Number.isFinite(den) && den > 0) return num / den
  }
  const parsed = Number(text)
  return Number.isFinite(parsed) ? parsed : 0
}

function extractTrackObjects(raw: unknown): Array<Record<string, unknown>> {
  if (!raw) return []
  if (Array.isArray(raw)) {
    return raw.filter((item): item is Record<string, unknown> => Boolean(item && typeof item === 'object' && !Array.isArray(item)))
  }
  if (typeof raw === 'object') {
    const obj = raw as Record<string, unknown>
    if (obj.audio) return extractTrackObjects(obj.audio)
    return [obj]
  }
  return []
}

function normalizeBilibiliBvid(raw: unknown): string {
  const text = typeof raw === 'string' ? raw.trim() : ''
  return /^bv[A-Za-z0-9]{4,32}$/i.test(text) ? `BV${text.slice(2)}` : ''
}

function normalizeBilibiliNumericId(raw: unknown, prefix: 'av' | 'ep' | 'ss'): string {
  if (typeof raw === 'number' && Number.isInteger(raw) && raw > 0) return String(raw)
  if (typeof raw !== 'string') return ''
  const match = new RegExp(`^(?:${prefix})?([1-9]\\d*)$`, 'i').exec(raw.trim())
  return match?.[1] || ''
}

function bilibiliPayloadConflictsWithPage(
  videoId: string,
  containers: Array<Record<string, unknown> | null | undefined>,
  streamUrls: Iterable<string> = [],
): boolean {
  const pageBvid = normalizeBilibiliBvid(videoId)
  const pageAid = /^av\d+$/i.test(videoId) ? normalizeBilibiliNumericId(videoId, 'av') : ''
  const pageEpId = /^ep\d+$/i.test(videoId) ? normalizeBilibiliNumericId(videoId, 'ep') : ''
  const pageSeasonId = /^ss\d+$/i.test(videoId) ? normalizeBilibiliNumericId(videoId, 'ss') : ''

  for (const obj of containers) {
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) continue
    if (pageBvid) {
      const payloadBvid = normalizeBilibiliBvid(obj.bvid ?? obj.bv_id)
      if (payloadBvid && payloadBvid !== pageBvid) return true
    }
    if (pageAid) {
      const payloadAid = normalizeBilibiliNumericId(obj.aid ?? obj.avid, 'av')
      if (payloadAid && payloadAid !== pageAid) return true
    }
    if (pageEpId) {
      const payloadEpId = normalizeBilibiliNumericId(obj.ep_id ?? obj.epid, 'ep')
      if (payloadEpId && payloadEpId !== pageEpId) return true
    }
    if (pageSeasonId) {
      const payloadSeasonId = normalizeBilibiliNumericId(obj.season_id ?? obj.ssid, 'ss')
      if (payloadSeasonId && payloadSeasonId !== pageSeasonId) return true
    }
  }

  if (pageBvid) {
    for (const rawUrl of streamUrls) {
      try {
        const parsed = new URL(rawUrl)
        const streamBvid = normalizeBilibiliBvid(parsed.searchParams.get('bvid'))
        if (streamBvid && streamBvid !== pageBvid) return true
      } catch {}
    }
  }
  return false
}

/** Extract Bilibili DASH/durl streams from `window.__playinfo__` or `playurl` JSON. */
export function extractBilibiliPlayinfo(pageUrl: string, body: unknown): SiteManifestCandidate[] {
  const videoId = bilibiliPageVideoId(pageUrl)
  if (!videoId) return []
  if (typeof body === 'string') {
    if (body.length > SITE_RESPONSE_LIMIT) return []
    try { body = JSON.parse(body) } catch { return [] }
  }
  if (!body || typeof body !== 'object' || Array.isArray(body)) return []
  const root = body as Record<string, unknown>
  const dataObj = root.data && typeof root.data === 'object' && !Array.isArray(root.data)
    ? root.data as Record<string, unknown>
    : null
  const resultObj = root.result && typeof root.result === 'object' && !Array.isArray(root.result)
    ? root.result as Record<string, unknown>
    : null
  const payload = (dataObj?.video_info && typeof dataObj.video_info === 'object' && !Array.isArray(dataObj.video_info) ? dataObj.video_info : null)
    || (resultObj?.video_info && typeof resultObj.video_info === 'object' && !Array.isArray(resultObj.video_info) ? resultObj.video_info : null)
    || dataObj
    || resultObj
    || root
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return []
  const data = payload as Record<string, unknown>
  const epInfoObj = (resultObj?.ep_info && typeof resultObj.ep_info === 'object' && !Array.isArray(resultObj.ep_info)
    ? resultObj.ep_info as Record<string, unknown>
    : null)
    || (dataObj?.ep_info && typeof dataObj.ep_info === 'object' && !Array.isArray(dataObj.ep_info)
      ? dataObj.ep_info as Record<string, unknown>
      : null)
  const viewInfoObj = (dataObj?.view_info && typeof dataObj.view_info === 'object' && !Array.isArray(dataObj.view_info)
    ? dataObj.view_info as Record<string, unknown>
    : null)
    || (resultObj?.view_info && typeof resultObj.view_info === 'object' && !Array.isArray(resultObj.view_info)
      ? resultObj.view_info as Record<string, unknown>
      : null)
  if (bilibiliPayloadConflictsWithPage(videoId, [root, dataObj, resultObj, data, epInfoObj, viewInfoObj])) return []
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
      fps: number
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
      const fps = parseFrameRate(track.frameRate ?? track.frame_rate)
      const highFps = fps >= 50 ? Math.round(fps) : 0
      const codecs = String(track.codecs || '')
      const id = String(track.id || '')
      const quality = height > 0
        ? `${height}p${highFps ? highFps : ''}`
        : (id ? `Q${id}` : '')
      parsedVideos.push({
        url: urls[0],
        width,
        height,
        bandwidth,
        fps,
        quality,
        codecs,
        variantId: `${id || height || 0}:${codecs || bandwidth || 0}`,
      })
    }

    type ParsedAudio = {
      url: string
      bandwidth: number
      codecs: string
      standardAac: boolean
      label: string
    }
    const parsedAudios: ParsedAudio[] = []
    const collectAudioGroup = (rawGroup: unknown, groupKind: 'standard' | 'dolby' | 'flac') => {
      for (const track of extractTrackObjects(rawGroup)) {
        const urls = bilibiliTrackUrls(track)
        if (!urls.length) continue
        urls.forEach(u => playbackUrls.add(u))
        const bandwidth = Math.max(0, Math.floor(Number(track.bandwidth || 0)))
        const codecs = String(track.codecs || track.mimeType || '')
        const lowerCodecs = codecs.toLowerCase()
        const isSpecialCodec = groupKind !== 'standard'
          || lowerCodecs.includes('flac')
          || lowerCodecs.includes('ec-3')
          || lowerCodecs.includes('ac-3')
          || lowerCodecs.includes('eac3')
        parsedAudios.push({
          url: urls[0],
          bandwidth,
          codecs,
          standardAac: !isSpecialCodec,
          label: String(codecs || track.id || (groupKind === 'flac' ? 'Hi-Res FLAC' : groupKind === 'dolby' ? 'Dolby' : '音频')),
        })
      }
    }
    collectAudioGroup(dash.audio, 'standard')
    collectAudioGroup(dash.dolby, 'dolby')
    collectAudioGroup(dash.flac, 'flac')

    // Prefer standard AAC .m4s (highest bandwidth) for universal MP4 muxing;
    // fall back to flac/dolby highest-bandwidth track only when standard AAC is absent.
    const standardAudios = parsedAudios
      .filter(track => track.standardAac)
      .sort((a, b) => b.bandwidth - a.bandwidth)
    const fallbackAudios = parsedAudios
      .filter(track => !track.standardAac)
      .sort((a, b) => b.bandwidth - a.bandwidth)
    const orderedAudios = [...standardAudios, ...fallbackAudios]
    const bestAudio = orderedAudios[0]
    const bestAudioUrl = bestAudio?.url || ''

    const audioTracks: MediaAudioTrack[] = orderedAudios.map((track, index) => ({
      label: track.label,
      language: '',
      url: track.url,
      default: index === 0,
    }))

    if (!parsedVideos.length) return []
    if (bilibiliPayloadConflictsWithPage(videoId, [], playbackUrls)) return []
    parsedVideos.sort((left, right) => right.height - left.height
      || Math.round(right.fps) - Math.round(left.fps)
      || codecPreference(right.codecs) - codecPreference(left.codecs)
      || right.bandwidth - left.bandwidth)
    // Keep one preferred codec per resolution/framerate tier so cards show clean quality choices.
    const byQuality = new Map<string, ParsedVideo>()
    for (const entry of parsedVideos) {
      const key = entry.quality || entry.variantId
      if (!byQuality.has(key)) byQuality.set(key, entry)
    }
    const chosen = [...byQuality.values()].slice(0, 8)
    const primary = chosen[0]
    const audioBandwidth = bestAudio?.bandwidth || 0
    const variants: MediaVariant[] = chosen.length > 1
      ? chosen.map(entry => ({
          url: entry.url,
          variantId: entry.variantId,
          ...(entry.width > 0 ? { width: entry.width } : {}),
          ...(entry.height > 0 ? { height: entry.height } : {}),
          ...(entry.bandwidth > 0 ? { bandwidth: entry.bandwidth + audioBandwidth } : {}),
          ...(entry.quality ? { quality: entry.quality } : {}),
          ...(bestAudioUrl ? { audioUrl: bestAudioUrl } : {}),
        }))
      : []
    const totalPrimaryBandwidth = primary.bandwidth > 0 ? primary.bandwidth + audioBandwidth : 0
    const estimatedSize = durationSeconds && totalPrimaryBandwidth > 0
      ? Math.round(durationSeconds * totalPrimaryBandwidth / 8)
      : undefined
    return [{
      url: primary.url,
      mimeType: 'video/mp4',
      videoId,
      ...(bestAudioUrl ? { audioUrl: bestAudioUrl } : {}),
      inspection: {
        inspected: true,
        isLive: false,
        ...(bestAudioUrl ? { audioUrl: bestAudioUrl, filename: `${videoId}.mp4` } : {}),
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
    if (bilibiliPayloadConflictsWithPage(videoId, [], urls)) return []
    const size = Math.max(0, Math.floor(Number((first as Record<string, unknown>).size || 0)))
    const itemLengthMs = Number((first as Record<string, unknown>).length || 0)
    const duration = durationSeconds || (Number.isFinite(itemLengthMs) && itemLengthMs > 0 ? Math.round(itemLengthMs / 1000) : undefined)
    const durlAudioCandidates = extractTrackObjects(data.durl_audio ?? (first as Record<string, unknown>).audio)
      .flatMap(track => {
        const trackUrls = bilibiliTrackUrls(track)
        return trackUrls.length
          ? [{ url: trackUrls[0], bandwidth: Math.max(0, Math.floor(Number(track.bandwidth || 0))) }]
          : []
      })
      .sort((a, b) => b.bandwidth - a.bandwidth)
    const durlAudioUrl = durlAudioCandidates[0]?.url || ''
    return [{
      url: urls[0],
      mimeType: 'video/mp4',
      videoId,
      ...(durlAudioUrl ? { audioUrl: durlAudioUrl } : {}),
      inspection: {
        inspected: true,
        isLive: false,
        ...(durlAudioUrl ? { audioUrl: durlAudioUrl, filename: `${videoId}.mp4` } : {}),
        ...(duration ? { duration } : {}),
        ...(size > 0 ? { estimatedSize: size } : {}),
        playbackUrls: [...urls, ...(durlAudioUrl ? [durlAudioUrl] : [])].slice(0, 48),
      },
    }]
  }

  return []
}

export const bilibiliPlayinfoCandidates = extractBilibiliPlayinfo

export function siteManifestCandidates(url: string, pageUrl: string, body: unknown): SiteManifestCandidate[] {
  if (!shouldInspectSiteResponse(url, pageUrl)) return []
  const biliVideoId = bilibiliPageVideoId(pageUrl)
  if (biliVideoId) {
    try {
      const req = new URL(url)
      const queryIds: Record<string, unknown> = {
        bvid: req.searchParams.get('bvid') ?? undefined,
        aid: req.searchParams.get('aid') ?? req.searchParams.get('avid') ?? undefined,
        ep_id: req.searchParams.get('ep_id') ?? req.searchParams.get('epid') ?? undefined,
        season_id: req.searchParams.get('season_id') ?? req.searchParams.get('ssid') ?? undefined,
      }
      if (bilibiliPayloadConflictsWithPage(biliVideoId, [queryIds])) return []
    } catch {
      return []
    }
    return extractBilibiliPlayinfo(pageUrl, body)
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
