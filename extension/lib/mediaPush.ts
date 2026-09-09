export function mediaPushRequestId(response: any, label: string): string {
  if (!response?.ok) throw new Error(response?.error || `${label}失败`)
  const id = typeof response.id === 'string' ? response.id.trim() : ''
  if (!id) throw new Error(`桌面端没有返回${label}请求 ID`)
  return id
}
