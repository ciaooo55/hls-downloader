import { isRunningStatus } from './taskState'

const STATUS_LABELS: Record<string, string> = {
  queued: '排队中',
  downloading: '准备下载',
  downloading_m3u8: '获取清单',
  parsing: '解析中',
  downloading_segments: '下载分片',
  pausing: '正在暂停',
  paused: '已暂停',
  merging: '合并中',
  remuxing: '转封装',
  done: '已完成',
  failed: '失败',
  canceled: '已取消',
  unsupported: '不支持',
  interrupted: '上次运行中断',
}

const STAGE_LABELS: Record<string, string> = {
  queued: '等待开始',
  downloading: '准备下载',
  downloading_m3u8: '获取播放清单',
  parsing: '解析播放清单',
  downloading_segments: '下载媒体分片',
  pausing: '等待当前分片完成',
  paused: '已暂停',
  merging: '合并视频',
  remuxing: '转封装',
  done: '已完成',
  failed: '下载失败',
  canceled: '已取消',
  unsupported: '格式不支持',
  interrupted: '上次运行中断',
}

export const statusLabel = (status: string) => STATUS_LABELS[status] || status || '--'
export const stageLabel = (stage: string) => STAGE_LABELS[stage] || stage || '--'

const ACTIVE = new Set([
  'queued', 'downloading', 'downloading_m3u8', 'parsing',
  'downloading_segments', 'pausing', 'paused', 'merging', 'remuxing',
])

export function filterAndSortTasks<T extends Record<string, any>>(
  tasks: T[],
  filter: string,
  query: string,
): T[] {
  const needle = query.trim().toLocaleLowerCase()
  return tasks.filter(task => {
    if (filter === 'running' && !(isRunningStatus(task.status) || task.status === 'queued')) return false
    if (filter !== 'all' && filter !== 'running' && task.status !== filter) return false
    if (!needle) return true
    return [task.id, task.title, task.filename, task.url, task.error_code, task.error_message]
      .some(value => String(value || '').toLocaleLowerCase().includes(needle))
  }).sort((a, b) => {
    const activeDifference = Number(ACTIVE.has(b.status)) - Number(ACTIVE.has(a.status))
    if (activeDifference) return activeDifference
    const createdDifference = String(b.created_at || '').localeCompare(String(a.created_at || ''))
    return createdDifference || String(a.id).localeCompare(String(b.id))
  })
}
