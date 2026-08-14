const DOWNLOAD_HINT = /(?:^|[\s_:/-])(?:download|export|install|offline)(?:$|[\s_:/-])|(?:^|[\s_:/-])save[\s_-]*(?:as|file|video|audio|media|download|offline)(?:$|[\s_:/-])|下载|下載|另存|保存(?:到)?(?:本地|文件|檔案|档案|视频|影片|音[频頻])|儲存(?:到)?(?:本[機机地]|檔案|文件)|导出|匯出|安装|安裝|离线|離線|缓存|ダウンロード|다운로드|скачать|télécharger|descargar|herunterladen|scarica|baixar/i
const DOWNLOAD_PATH_EXT = /\.(?:m3u8?|mpd|mp4|m4v|webm|mkv|mov|avi|flv|f4v|3gp|m4a|mp3|flac|wav|ogg|opus|aac|torrent|metalink|meta4|zip|7z|rar|tar|tgz|gz|bz2|xz|iso|img|exe|msi|msix|appx|apk|dmg|pkg|deb|rpm|pdf|epub|docx?|xlsx?|pptx?|bin|jar)(?:$|[?#])/i
const MEDIA_PATH_EXT = /\.(?:m3u8?|mpd|mp4|m4v|webm|mkv|mov|avi|flv|f4v|3gp|m4a|mp3|flac|wav|ogg|opus|aac)(?:$|[?#])/i
const DOWNLOAD_PATH_SEGMENT = /\/(?:downloads?|attachments?|exports?)(?:\/|$)/i
const PLAYBACK_PAGE_PATH = /(?:^|\/)(?:watch|shorts?|play(?:er|ing|back)?|videos?|episode(?:s)?|view(?:_video)?|movies?|films?|vod|clips?|listen|tracks?|embed|live)(?:\/|$|\.[a-z0-9]+)/i
const NEGATIVE_DOWNLOAD_FLAGS = new Set(['0', 'false', 'no', 'off', 'none', 'null', 'undefined', 'n', 'f'])
const POSITIVE_DOWNLOAD_FLAGS = new Set(['1', 'true', 'yes', 'on', 'force', 'attachment', 'dl', 'download', 'save', 'export'])
const FLAG_QUERY_KEYS = ['download', 'attachment', 'disposition'] as const
const FILENAME_QUERY_KEYS = ['filename', 'file_name', 'fn', 'file'] as const
const AUTHENTICATION_HOSTS = new Set([
  'accounts.google.com',
  'login.live.com',
  'login.microsoftonline.com',
  'login.microsoft.com',
])
const AUTHENTICATION_PATH = /\/(?:o\/oauth2(?:\/|$)|oauth2?(?:\/|$)|openid(?:\/|$)|login\/oauth(?:\/|$)|connect\/authorize(?:\/|$)|authorize(?:\/|$))/i
const OAUTH_PARAMETERS = ['client_id', 'redirect_uri', 'response_type', 'scope', 'state', 'code_challenge', 'nonce']

export function isLikelyDownloadControl(hints: Array<string | null | undefined>): boolean {
  const value = hints.filter(Boolean).join(' ').replace(/([a-z])([A-Z])/g, '$1 $2')
  return DOWNLOAD_HINT.test(` ${value} `)
}

/**
 * Authentication pages must always remain browser navigations.  In particular,
 * OAuth may use a normal anchor followed by several redirects, which looks
 * superficially like a download gateway if every click is remembered.
 */
export function isAuthenticationNavigation(value?: string): boolean {
  if (!value) return false
  try {
    const url = new URL(value)
    const host = url.hostname.toLowerCase()
    if (AUTHENTICATION_HOSTS.has(host)) return true
    const parameterCount = OAUTH_PARAMETERS.filter(name => url.searchParams.has(name)).length
    const hasOAuthHandshake = url.searchParams.has('client_id')
      && (url.searchParams.has('redirect_uri') || url.searchParams.has('response_type') || url.searchParams.has('code_challenge'))
    return (AUTHENTICATION_PATH.test(url.pathname) && parameterCount >= 2) || hasOAuthHandshake
  } catch {
    return false
  }
}

function queryLeaf(value: string): string {
  const trimmed = value.trim()
  try {
    return decodeURIComponent(trimmed).split(/[\\/]/).pop() || trimmed
  } catch {
    return trimmed.split(/[\\/]/).pop() || trimmed
  }
}

function valueLooksLikeDownloadFile(value: string): boolean {
  return DOWNLOAD_PATH_EXT.test(queryLeaf(value))
}

function isNegativeDownloadFlag(value: string): boolean {
  return NEGATIVE_DOWNLOAD_FLAGS.has(value.trim().toLowerCase())
}

function isPositiveDownloadFlag(value: string): boolean {
  const normalized = value.trim().toLowerCase()
  return normalized === '' || POSITIVE_DOWNLOAD_FLAGS.has(normalized)
}

function hasDownloadQuery(url: URL): boolean {
  const playbackPage = PLAYBACK_PAGE_PATH.test(url.pathname)
  for (const key of FLAG_QUERY_KEYS) {
    if (!url.searchParams.has(key)) continue
    const raw = url.searchParams.get(key) || ''
    if (isNegativeDownloadFlag(raw)) continue
    if (isPositiveDownloadFlag(raw) || valueLooksLikeDownloadFile(raw)) return true
  }
  for (const key of ['cd', 'content-disposition', 'content_disposition']) {
    const raw = url.searchParams.get(key)
    if (raw && /^\s*attachment\b/i.test(raw)) return true
  }
  if (url.searchParams.has('export')) {
    const raw = url.searchParams.get('export') || ''
    if (!isNegativeDownloadFlag(raw) && (isPositiveDownloadFlag(raw) || valueLooksLikeDownloadFile(raw))) return true
  }
  for (const key of FILENAME_QUERY_KEYS) {
    if (!url.searchParams.has(key)) continue
    const raw = url.searchParams.get(key) || ''
    if (!valueLooksLikeDownloadFile(raw)) continue
    // Watch/player pages often pass the playing media name in `file=` / `fn=`.
    // That is navigation, not a download click. Archives and installers still count.
    if (playbackPage && MEDIA_PATH_EXT.test(queryLeaf(raw))) continue
    return true
  }
  return false
}

/** True for a concrete URL that is independently download-looking. */
export function isLikelyDownloadUrl(value?: string): boolean {
  if (!value || isAuthenticationNavigation(value)) return false
  try {
    const url = new URL(value)
    if (!['http:', 'https:', 'magnet:'].includes(url.protocol)) return false
    if (url.protocol === 'magnet:') return true
    return DOWNLOAD_PATH_EXT.test(url.pathname)
      || DOWNLOAD_PATH_SEGMENT.test(url.pathname)
      || hasDownloadQuery(url)
  } catch {
    return false
  }
}

/**
 * A page may expose a download as a normal link, a JavaScript link, or a
 * button. Record only explicit download controls, download-looking targets,
 * or a user-forced Ctrl click. Ordinary navigation must not create a pending
 * intent: OAuth/login redirects otherwise get paired with an unrelated
 * download event in the same tab.
 */
export function shouldTrackDownloadIntent(input: {
  directHref?: string
  hintedHref?: string
  ctrlForce?: boolean
  explicitDownloadTarget?: boolean
  hints?: Array<string | null | undefined>
}): boolean {
  if (isAuthenticationNavigation(input.directHref) || isAuthenticationNavigation(input.hintedHref)) return false
  return Boolean(
    input.explicitDownloadTarget
    || isLikelyDownloadUrl(input.directHref)
    || isLikelyDownloadUrl(input.hintedHref)
    || input.ctrlForce
    || isLikelyDownloadControl(input.hints || []),
  )
}

/** Resolve only resource schemes that the desktop downloader can own. */
export function resolveDownloadTarget(value: string, baseUrl: string): string {
  const raw = String(value || '').trim()
  if (!raw || raw.startsWith('#') || /^javascript:/i.test(raw)) return ''
  try {
    const target = new URL(raw, baseUrl)
    return ['http:', 'https:', 'magnet:'].includes(target.protocol) ? target.href : ''
  } catch {
    return ''
  }
}

function usableHrefAttribute(value = ''): string {
  const raw = String(value || '').trim()
  if (!raw || raw.startsWith('#') || /^javascript:/i.test(raw)) return ''
  return raw
}

/** HTML `<a>`/`<area>` use `.href`; SVG `<a>` exposes href as an animated string. */
export function resolveClickedLinkHref(input: {
  htmlHref?: string
  htmlHrefAttribute?: string
  svgHrefAttribute?: string
  svgXlinkHref?: string
  svgBaseVal?: string
  baseUrl: string
}): string {
  const htmlAttribute = usableHrefAttribute(input.htmlHrefAttribute)
  if (htmlAttribute && input.htmlHref) {
    return resolveDownloadTarget(input.htmlHref, input.baseUrl) || String(input.htmlHref)
  }
  return resolveDownloadTarget(
    usableHrefAttribute(input.svgHrefAttribute || input.svgXlinkHref || input.svgBaseVal),
    input.baseUrl,
  )
}

/**
 * Submit buttons often put the file on `formaction` or the form `action`
 * instead of an `<a href>`. Only keep targets that already look like downloads
 * so a login form cannot become a pending intent.
 */
export function resolveFormDownloadUrl(formActionAttribute: string, submitFormAction: string, baseUrl: string): string {
  const raw = usableHrefAttribute(submitFormAction) || usableHrefAttribute(formActionAttribute)
  const resolved = resolveDownloadTarget(raw, baseUrl)
  return isLikelyDownloadUrl(resolved) ? resolved : ''
}

export function linkOpensNewTab(target = ''): boolean {
  return String(target || '').trim().toLowerCase() === '_blank'
}
