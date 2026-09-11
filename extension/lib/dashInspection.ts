import { replayableRequestHeaders, type MediaResource } from './resources'
import { readBoundedResponseText } from './boundedResponse'

export type DashManifestFetcher = (url: string, init: RequestInit) => Promise<Response>

export interface DashInspectionResult {
  inspected: true
  isLive: boolean
  duration?: number
  width?: number
  height?: number
  bandwidth?: number
  estimatedSize?: number
  quality?: string
  playbackUrls: string[]
  playbackPatterns: string[]
}

interface DashRepresentation {
  id: string
  width: number
  height: number
  bandwidth: number
}

interface DashCandidate extends DashRepresentation {
  periodBody: string
  adaptationBody: string
  representationBody: string
}

function attributes(value: string): Record<string, string> {
  const result: Record<string, string> = {}
  const pattern = /([:\w.-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g
  for (const match of value.matchAll(pattern)) result[match[1].toLowerCase()] = match[2] ?? match[3] ?? ''
  return result
}

function positiveFiniteNumber(value = ''): number {
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0
}

function estimatedBytes(duration: number | undefined, bandwidth: number): number | undefined {
  if (!Number.isFinite(duration) || (duration || 0) <= 0 || !Number.isFinite(bandwidth) || bandwidth <= 0) return undefined
  const bytes = (duration || 0) * bandwidth / 8
  return Number.isFinite(bytes) && bytes > 0 ? Math.round(bytes) : undefined
}

function isoDuration(value = ''): number | null {
  const match = value.match(/^P(?:(\d+(?:\.\d+)?)D)?(?:T(?:(\d+(?:\.\d+)?)H)?(?:(\d+(?:\.\d+)?)M)?(?:(\d+(?:\.\d+)?)S)?)?$/i)
  if (!match || match.slice(1).every(part => part === undefined)) return null
  const seconds = (Number(match[1]) || 0) * 86_400
    + (Number(match[2]) || 0) * 3_600
    + (Number(match[3]) || 0) * 60
    + (Number(match[4]) || 0)
  return Number.isFinite(seconds) ? seconds : null
}

function decodeXml(value: string): string {
  const named: Record<string, string> = {
    amp: '&',
    quot: '"',
    apos: "'",
    lt: '<',
    gt: '>',
  }
  return value.replace(/&(?:#(x[0-9a-f]+|\d+)|(amp|quot|apos|lt|gt));/gi, (entity, encoded?: string, namedEntity?: string) => {
    if (namedEntity) return named[namedEntity.toLowerCase()] || entity
    if (!encoded) return entity
    const radix = encoded[0]?.toLowerCase() === 'x' ? 16 : 10
    const digits = radix === 16 ? encoded.slice(1) : encoded
    const codePoint = Number.parseInt(digits, radix)
    if (!Number.isInteger(codePoint) || codePoint < 0 || codePoint > 0x10ffff
      || (codePoint >= 0xd800 && codePoint <= 0xdfff)) return entity
    return String.fromCodePoint(codePoint)
  })
}

function xmlName(tag: string): string {
  return `(?:[A-Za-z_][\\w.-]*:)?${tag}`
}

function blocks(value: string, tag: string): Array<{ attributes: Record<string, string>; body: string }> {
  const result: Array<{ attributes: Record<string, string>; body: string }> = []
  const name = xmlName(tag)
  const pattern = new RegExp(`<${name}\\b([^>]*?)(?:\\/\\s*>|>([\\s\\S]*?)<\\/${name}\\s*>)`, 'gi')
  for (const match of value.matchAll(pattern)) {
    result.push({ attributes: attributes(match[1]), body: match[2] || '' })
  }
  return result
}

function withoutBlocks(value: string, tag: string): string {
  const name = xmlName(tag)
  return value.replace(
    new RegExp(`<${name}\\b[^>]*?(?:\\/\\s*>|>[\\s\\S]*?<\\/${name}\\s*>)`, 'gi'),
    '',
  )
}

function directBaseUrl(value: string): string {
  const name = xmlName('BaseURL')
  const match = value.match(new RegExp(`<${name}(?:\\s[^>]*)?>([\\s\\S]*?)<\\/${name}\\s*>`, 'i'))
  return decodeXml(match?.[1] || '').trim()
}

function directSegmentTemplate(value: string): Record<string, string> | null {
  const match = value.match(new RegExp(`<${xmlName('SegmentTemplate')}\\b([^>]*)>`, 'i'))
  return match ? attributes(match[1]) : null
}

function resolveBase(value: string, baseUrl: string): string {
  if (!value) return baseUrl
  try { return new URL(value, baseUrl).href } catch { return baseUrl }
}

function resolveHint(value: string, baseUrl: string, representationId = ''): string {
  const substituted = decodeXml(value.trim())
    .replace(/\$RepresentationID\$/gi, representationId)
    .replace(/\$(?:Number|Time|Bandwidth)(?:%0\d+d)?\$/gi, '0')
    .replace(/\$\$/g, '$')
  if (!substituted) return ''
  try { return new URL(substituted, baseUrl).href } catch { return '' }
}

function resolvePattern(value: string, baseUrl: string, representationId = ''): string {
  const substituted = decodeXml(value.trim())
    .replace(/\$RepresentationID\$/gi, representationId)
    .replace(/\$(?:Number|Time|Bandwidth)(?:%0\d+d)?\$/gi, '*')
    .replace(/\$\$/g, '$')
  if (!substituted) return ''
  try { return new URL(substituted, baseUrl).href } catch { return '' }
}

/** Parse the MPD metadata needed by the browser panel without a DOM dependency. */
export function parseDashManifest(text: string, baseUrl: string): DashInspectionResult | null {
  const rootName = xmlName('MPD')
  const root = text.match(new RegExp(`<${rootName}\\b([^>]*)>([\\s\\S]*?)<\\/${rootName}\\s*>`, 'i'))
  if (!root) return null
  const rootAttributes = attributes(root[1])
  const rootBody = root[2]
  const isLive = rootAttributes.type?.toLowerCase() === 'dynamic'
  const periodBlocks = blocks(rootBody, 'Period')
  const periods = periodBlocks.length ? periodBlocks : [{ attributes: {}, body: rootBody }]
  const rootDuration = isoDuration(rootAttributes.mediapresentationduration)
  const periodDurations = periods.map(period => isoDuration(period.attributes.duration))
  const explicitPeriodDuration = periodDurations.length > 0 && periodDurations.every(value => value !== null)
    ? (() => {
        const total = periodDurations.reduce((sum, value) => sum + (value || 0), 0)
        return Number.isFinite(total) ? total : null
      })()
    : null
  const duration = isLive ? undefined : (rootDuration ?? explicitPeriodDuration ?? 0)

  const video: DashCandidate[] = []
  const audioBandwidth: number[] = []
  for (const period of periods) {
    for (const adaptationBlock of blocks(period.body, 'AdaptationSet')) {
      const adaptation = adaptationBlock.attributes
      const kind = `${adaptation.contenttype || ''} ${adaptation.mimetype || ''}`.toLowerCase()
      const representations = blocks(adaptationBlock.body, 'Representation')
      const candidates = representations.length
        ? representations.map(representation => ({
            attributes: { ...adaptation, ...representation.attributes },
            body: representation.body,
          }))
        : [{ attributes: adaptation, body: '' }]
      for (const candidate of candidates) {
        const mime = `${kind} ${candidate.attributes.contenttype || ''} ${candidate.attributes.mimetype || ''}`.toLowerCase()
        const bandwidth = positiveFiniteNumber(candidate.attributes.bandwidth)
        const width = positiveFiniteNumber(candidate.attributes.width)
        const height = positiveFiniteNumber(candidate.attributes.height)
        if (mime.includes('video') || width > 0 || height > 0) {
          video.push({
            id: candidate.attributes.id || '',
            width,
            height,
            bandwidth,
            periodBody: period.body,
            adaptationBody: adaptationBlock.body,
            representationBody: candidate.body,
          })
        } else if (mime.includes('audio') && bandwidth > 0) {
          audioBandwidth.push(bandwidth)
        }
      }
    }
  }
  const best = video.sort((left, right) => right.height - left.height || right.bandwidth - left.bandwidth)[0]
  const maxAudioBandwidth = audioBandwidth.reduce((maximum, value) => Math.max(maximum, value), 0)
  const combinedBandwidth = (best?.bandwidth || 0) + maxAudioBandwidth
  const totalBandwidth = Number.isFinite(combinedBandwidth) ? combinedBandwidth : 0

  const hints: string[] = []
  const patterns: string[] = []
  const rootDirect = withoutBlocks(rootBody, 'Period')
  const periodDirect = best ? withoutBlocks(best.periodBody, 'AdaptationSet') : ''
  const adaptationDirect = best ? withoutBlocks(best.adaptationBody, 'Representation') : ''
  let mediaBase = baseUrl
  for (const scope of [rootDirect, periodDirect, adaptationDirect, best?.representationBody || '']) {
    mediaBase = resolveBase(directBaseUrl(scope), mediaBase)
  }
  const remember = (value: string) => {
    const resolved = resolveHint(value, mediaBase, best?.id)
    if (resolved && !hints.includes(resolved)) hints.push(resolved)
  }
  if (mediaBase !== baseUrl && !mediaBase.endsWith('/')) remember(mediaBase)
  const templateScopes = [best?.representationBody || '', adaptationDirect, periodDirect, rootDirect]
  const template = templateScopes.map(directSegmentTemplate).find(Boolean)
  if (template) {
    remember(template.initialization || '')
    const pattern = resolvePattern(template.media || '', mediaBase, best?.id)
    if (pattern && !patterns.includes(pattern)) patterns.push(pattern)
  }

  return {
    inspected: true,
    isLive,
    duration: duration || undefined,
    width: best?.width || undefined,
    height: best?.height || undefined,
    bandwidth: best?.bandwidth || undefined,
    estimatedSize: estimatedBytes(duration, totalBandwidth),
    quality: best?.height ? `最高 ${best.height}p` : undefined,
    playbackUrls: hints.slice(0, 48),
    playbackPatterns: patterns.slice(0, 48),
  }
}

export async function inspectDashResource(
  resource: Pick<MediaResource, 'url' | 'requestHeaders'> & {
    inspectionHeaders?: Record<string, string>
  },
  fetcher: DashManifestFetcher = (url, init) => fetch(url, init),
): Promise<DashInspectionResult | null> {
  const headers = replayableRequestHeaders(resource.requestHeaders)
  for (const name of Object.keys(headers)) {
    if (['user-agent'].includes(name) || name.startsWith('sec-')) delete headers[name]
  }
  for (const [rawName, rawValue] of Object.entries(resource.inspectionHeaders || {})) {
    const name = String(rawName || '').trim().toLowerCase()
    const value = String(rawValue || '').trim()
    if (!['referer', 'origin', 'cookie'].includes(name) || !value || /[\r\n]/.test(value) || value.length > 32 * 1024) continue
    headers[name] = value
  }
  const fallbackHeaders = replayableRequestHeaders(resource.requestHeaders)
  for (const name of Object.keys(fallbackHeaders)) {
    if (['user-agent'].includes(name) || name.startsWith('sec-')) delete fallbackHeaders[name]
  }
  const init = { credentials: 'include' as const, headers, signal: AbortSignal.timeout(5_000) }
  let response: Response
  try {
    response = await fetcher(resource.url, init)
    if (!response.ok && Object.keys(resource.inspectionHeaders || {}).length) {
      response = await fetcher(resource.url, { ...init, headers: fallbackHeaders })
    }
  } catch (error) {
    if (!Object.keys(resource.inspectionHeaders || {}).length) throw error
    response = await fetcher(resource.url, { ...init, headers: fallbackHeaders })
  }
  if (!response.ok) return null
  const text = await readBoundedResponseText(response, 2 * 1024 * 1024)
  if (text === null) return null
  return parseDashManifest(text, response.url || resource.url)
}
