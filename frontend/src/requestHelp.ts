export const REQUEST_EXAMPLES = {
  referer: 'https://example.com/watch/123',
  origin: 'https://example.com',
  userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0',
  cookie: '',
  ffmpegPath: 'bin\\ffmpeg.exe',
} as const

export const REQUEST_FIELD_HELP = {
  referer: `仅作为手工或批量任务的默认值。填写资源所在页面的完整地址，例如 ${REQUEST_EXAMPLES.referer}；不确定时留空，不要套用其他站点。浏览器插件任务使用实际捕获值。`,
  origin: `仅作为手工或批量任务的默认值。只填“协议 + 域名”，例如 ${REQUEST_EXAMPLES.origin}，不含路径且末尾不加 /；不确定时留空。浏览器插件不会凭空生成 Origin。`,
  userAgent: `模拟浏览器身份，通常保留默认值即可。示例：${REQUEST_EXAMPLES.userAgent}`,
  cookie: '仅作为手工或批量任务的默认值。登录资源应优先通过浏览器插件授权当前站点 Cookie；手工填写时不要带 Cookie: 前缀。',
  ffmpegPath: `用于合并和检查视频。安装版保持 ${REQUEST_EXAMPLES.ffmpegPath} 即可；使用外部 FFmpeg 时填写 ffmpeg.exe 的完整路径。`,
  allowedHosts: '限制下载器可访问的网站域名，多个域名用英文逗号分隔，例如 example.com,cdn.example.com。旧版默认留空，表示不限制。',
  concurrency: '单个任务同时下载的分片数量。默认 12，最高 256；过高可能触发网站限速或占用更多连接。普通文件服务器不支持 Range 时自动使用单连接。',
  maxTasks: '同时处于下载状态的任务数量，其余任务排队等待（默认 3）。排队任务可在列表右键调整优先级。',
  speedLimit: '全局下载限速（KiB/s）。0 表示不限速；HTTP/HLS 分片共享该预算，适合网络受限时控制带宽。',
} as const
