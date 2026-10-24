// 只提前接管明确的附件链接；PDF、图片和媒体的普通预览仍由响应头阶段判断。
const ATTACHMENT_PATH = /\.(?:zip|7z|rar|tar|tgz|gz|bz2|xz|iso|img|exe|msi|msix|appx|apk|dmg|pkg|deb|rpm|epub|docx?|xlsx?|pptx?|vsix|nupkg|cab|bin|jar|torrent|metalink|meta4)$/i

export function isDirectDownloadLink(href: string, pageUrl: string, downloadAttribute: boolean): boolean {
  try {
    const url = new URL(href)
    if (!['http:', 'https:'].includes(url.protocol)) return false
    // GitHub 的源码查看页不是附件，尽管路径也可能以 .zip/.exe 结尾。
    if (url.hostname === 'github.com' && /^\/[^/]+\/[^/]+\/blob\//.test(url.pathname)) return false
    return ATTACHMENT_PATH.test(url.pathname)
      || (downloadAttribute && url.origin === new URL(pageUrl).origin)
  } catch {
    return false
  }
}
