export const LEGACY_REQUEST_EXAMPLES = {
  referer: 'https://missav.ai/',
  origin: 'https://missav.ai',
  userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0',
  cookie: '',
  ffmpegPath: 'bin\\ffmpeg.exe',
} as const

export const REQUEST_FIELD_HELP = {
  referer: `用于网站防盗链校验，填写视频所在页面或站点的完整地址。旧版示例：${LEGACY_REQUEST_EXAMPLES.referer}；不需要时留空。`,
  origin: `表示请求来源，只填“协议 + 域名”，不含路径，也不要在末尾加 /。旧版示例：${LEGACY_REQUEST_EXAMPLES.origin}；不需要时留空。`,
  userAgent: `模拟浏览器身份，通常保留默认值即可。旧版示例：${LEGACY_REQUEST_EXAMPLES.userAgent}`,
  cookie: '登录、会员或年龄验证资源可能需要。从浏览器开发者工具复制 Cookie 请求头的值，不要填 Cookie: 前缀；格式如 sessionid=abc; token=xyz。旧版默认留空。',
  ffmpegPath: `用于合并和检查视频。安装版保持 ${LEGACY_REQUEST_EXAMPLES.ffmpegPath} 即可；使用外部 FFmpeg 时填写 ffmpeg.exe 的完整路径。`,
  allowedHosts: '限制下载器可访问的网站域名，多个域名用英文逗号分隔，例如 example.com,cdn.example.com。旧版默认留空，表示不限制。',
  concurrency: '单个任务同时下载的分片数量。默认 12，最高 256；过高可能触发网站限速或占用更多连接。普通文件服务器不支持 Range 时自动使用单连接。',
  maxTasks: '同时处于下载状态的任务数量，其余任务排队等待。当前默认 3。',
} as const
