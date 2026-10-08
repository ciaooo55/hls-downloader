import { withDeadline } from './asyncDeadline'

// 网页按钮和工具栏弹窗都等桌面确认，不能把“请求已收到”显示成“已下载”。
export async function waitForDownloadConfirmation(
  send: (message: Record<string, unknown>) => Promise<any>,
  handoffId: string,
): Promise<boolean> {
  if (!handoffId) throw new Error('桌面端未返回下载确认请求')
  const deadline = Date.now() + 125_000
  while (Date.now() < deadline) {
    const response = await withDeadline(send({ type: 'handoff-status', handoffId }), 3_000, '下载器响应超时，请重试')
    if (!response?.ok) throw new Error(response?.error || '无法读取下载确认状态')
    const status = response.handoff?.status
    if (status === 'accepted') return true
    if (status === 'rejected' || status === 'canceled') return false
    if (status !== 'pending') throw new Error(response.handoff?.presentation_error || '下载确认已失效，请重试')
    if (response.handoff?.presentation === 'fallback' || response.handoff?.presentation === 'failed') {
      throw new Error(response.handoff?.presentation_error || '无法显示下载确认窗口，请打开主程序后重试')
    }
    await new Promise(resolve => setTimeout(resolve, 600))
  }
  throw new Error('尚未确认下载，请在桌面窗口处理后重试')
}
