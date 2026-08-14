const DOWNLOAD_HINT = /(?:^|[\s_:/-])(?:download|save|export|install|offline)(?:$|[\s_:/-])|下载|保存|另存|导出|安装|离线|缓存|ダウンロード|다운로드|скачать|télécharger|descargar|herunterladen|scarica|baixar/i
const DOWNLOAD_URL_HINT = /\.(?:m3u8|mpd|mp4|m4v|webm|mkv|mov|avi|flv|m4a|mp3|flac|wav|ogg|opus|torrent|zip|7z|rar|tar|tgz|gz|bz2|xz|iso|img|exe|msi|msix|appx|apk|dmg|pkg|deb|rpm|pdf|epub|docx?|xlsx?|pptx?|bin|jar)(?:$|[?#])|\/(?:downloads?|attachments?|exports?)(?:[/?#]|$)|[?&](?:download|attachment|disposition|filename|file_name|fn|export)=|[?&](?:cd|content-disposition)=attachment(?:[&#]|$)/i
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

/** True for a concrete URL that is independently download-looking. */
export function isLikelyDownloadUrl(value?: string): boolean {
  if (!value || isAuthenticationNavigation(value)) return false
  try {
    const url = new URL(value)
    return ['http:', 'https:', 'magnet:'].includes(url.protocol) && (url.protocol === 'magnet:' || DOWNLOAD_URL_HINT.test(url.href))
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
  try {
    const target = new URL(String(value || '').trim(), baseUrl)
    return ['http:', 'https:', 'magnet:'].includes(target.protocol) ? target.href : ''
  } catch {
    return ''
  }
}
