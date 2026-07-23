const stageLabels: Record<string, string> = {
  downloading_m3u8: '获取播放清单',
  parsing: '解析播放清单',
  downloading_segments: '下载分片',
  merging: '合并文件',
  remuxing: '转封装',
  unsupported: '格式检查',
  downloading: '下载文件',
}

type FailureSource = {
  error_code?: string
  error_stage?: string
  error_url?: string
  error_hint?: string
  error_message?: string
  http_status?: number
  error_attempt?: number
}

export function getFailureDetails(task: FailureSource) {
  const items: Array<{ label: string; value: string }> = []
  if (task.error_stage) {
    items.push({ label: '发生阶段', value: stageLabels[task.error_stage] || task.error_stage })
  }
  if (task.http_status) items.push({ label: 'HTTP 状态', value: String(task.http_status) })
  if (task.error_code) items.push({ label: '错误代码', value: task.error_code })
  if (task.error_attempt) items.push({ label: '尝试次数', value: `${task.error_attempt} 次` })
  if (task.error_url) items.push({ label: '资源地址', value: task.error_url })

  const steps: string[] = []
  const code = task.error_code || ''
  const hint = task.error_hint || ''
  if (code.includes('403') || task.http_status === 403) {
    steps.push('回到原网页刷新并重新打开目标视频/下载入口')
    steps.push('用浏览器扩展重新发送资源，不要只点“重试”')
    if (code.includes('CLOUDFLARE')) steps.push('若页面出现人机验证，先在浏览器完成验证再抓取')
    if (code.includes('SIGNATURE') || /签名|过期/.test(hint)) steps.push('签名链接过期后必须重新获取，旧 URL 无法恢复')
    steps.push('需要登录时，在扩展中授权当前站点 Cookie')
  } else if (task.http_status === 401) {
    steps.push('确认已登录原网站')
    steps.push('用扩展重新发送并授权 Cookie/令牌')
  } else if (task.http_status === 429) {
    steps.push('降低默认并发与同时任务数')
    steps.push('等待数分钟后再重试')
  } else if (hint) {
    steps.push(hint)
  }

  return {
    title: task.error_code ? `下载失败 · ${task.error_code}` : '下载失败',
    items,
    message: task.error_message || '',
    hint,
    steps,
  }
}
