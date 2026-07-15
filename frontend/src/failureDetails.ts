const stageLabels: Record<string, string> = {
  downloading_m3u8: '获取播放清单',
  parsing: '解析播放清单',
  downloading_segments: '下载分片',
  merging: '合并文件',
  remuxing: '转封装',
  unsupported: '格式检查',
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
  if (task.error_attempt) items.push({ label: '尝试次数', value: `${task.error_attempt} 次` })
  if (task.error_url) items.push({ label: '资源地址', value: task.error_url })

  return {
    title: task.error_code ? `下载失败 · ${task.error_code}` : '下载失败',
    items,
    message: task.error_message || '',
    hint: task.error_hint || '',
  }
}
