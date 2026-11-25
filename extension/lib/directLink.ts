// 只提前接管明确的附件与常见大文件下载链接；网页普通导航仍由响应头阶段判断。
const ATTACHMENT_PATH = /\.(?:zip|7z|rar|tar|tgz|gz|bz2|xz|zst|iso|img|exe|msi|msix|appx|apk|appimage|dmg|pkg|deb|rpm|pdf|epub|docx?|xlsx?|pptx?|vsix|nupkg|whl|safetensors|gguf|onnx|cab|bin|jar|mp4|mkv|webm|mov|avi|mp3|flac|wav|m4a|torrent|metalink|meta4)$/i

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
