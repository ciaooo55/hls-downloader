export interface HlsVariant {
  url: string
  width?: number
  height?: number
  bandwidth?: number
  quality?: string
}

export interface HlsManifestInfo {
  variants: HlsVariant[]
  duration?: number
}

function attribute(line: string, name: string): string {
  const match = line.match(new RegExp(`(?:^|,)${name}=("[^"]*"|[^,]*)`, 'i'))
  return (match?.[1] || '').replace(/^"|"$/g, '')
}

export function parseHlsManifest(text: string, baseUrl: string): HlsManifestInfo {
  const lines = text.split(/\r?\n/).map(line => line.trim()).filter(Boolean)
  const variants: HlsVariant[] = []
  let duration = 0
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]
    if (line.startsWith('#EXTINF:')) duration += Number(line.slice(8).split(',', 1)[0]) || 0
    if (!line.startsWith('#EXT-X-STREAM-INF:')) continue
    const uri = lines.slice(index + 1).find(value => !value.startsWith('#'))
    if (!uri) continue
    const attributes = line.slice('#EXT-X-STREAM-INF:'.length)
    const resolution = attribute(attributes, 'RESOLUTION').match(/^(\d+)x(\d+)$/i)
    const width = Number(resolution?.[1] || 0) || undefined
    const height = Number(resolution?.[2] || 0) || undefined
    const bandwidth = Number(attribute(attributes, 'BANDWIDTH')) || undefined
    variants.push({
      url: new URL(uri, baseUrl).href,
      width,
      height,
      bandwidth,
      quality: height ? `${height}p` : undefined,
    })
  }
  return { variants, duration: duration > 0 ? duration : undefined }
}

export function resourceQuality(url: string, height?: number): string {
  if (height) return `${height}p`
  const value = url.match(/(?:^|[\/_-])(2160|1440|1080|720|540|480|360|240)p?(?:[\/_?.-]|$)/i)?.[1]
  return value ? `${value}p` : ''
}
