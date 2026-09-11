export function mediaPushRequestId(response: any, label: string): string {
  if (!response?.ok) throw new Error(response?.error || `${label}失败`)
  const status = String(response?.status || '')
  if (status === 'failed' || status === 'canceled') {
    throw new Error(String(response?.message || `${label}未完成`))
  }
  const id = typeof response.id === 'string' ? response.id.trim() : ''
  if (!id) throw new Error(`桌面端没有返回${label}请求 ID`)
  return id
}

export function mediaPushTerminalResult(response: any): any | null {
  if (response?.ok === false) {
    return {
      status: 'failed',
      message: String(response.error || '读取桌面端推送状态失败'),
    }
  }
  const status = String(response?.status || '')
  return ['done', 'failed', 'canceled'].includes(status) ? response : null
}
