import { isRunningStatus } from './taskState'
import { downloadCategory } from './downloadCategory'
import { fmtBytes } from './format'

const STATUS_LABELS: Record<string, string> = {
  queued: '排队中',
  awaiting_confirmation: '等待确认',
  fetching_metadata: '获取 BT 元数据',
  awaiting_selection: '等待选择文件',
  checking: '校验文件',
  scanning: '病毒扫描',
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
  awaiting_confirmation: '等待接管确认',
  fetching_metadata: '获取 BT 元数据',
  awaiting_selection: '选择 BT 文件',
  checking: '校验 BT piece',
  downloading: '准备下载',
  downloading_m3u8: '获取播放清单',
  parsing: '解析播放清单',
  downloading_segments: '下载媒体分片',
  recording: '直播录制中',
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

export function taskStatusLabel(task: { status: string; output_missing?: boolean; is_live?: boolean; queue_position?: number }): string {
  if (task.status === 'done' && task.output_missing) return '文件已删除'
  if (task.status === 'queued' && task.queue_position) return `排队中 · 第 ${task.queue_position} 位`
  if (task.is_live && task.status === 'downloading_segments') return '直播录制'
  if (task.is_live && task.status === 'pausing') return '正在停止录制'
  return statusLabel(task.status)
}
export const stageLabel = (stage: string) => STAGE_LABELS[stage] || stage || '--'

/** Keep byte information visible in every task-table phase. A live stream
 * has no trustworthy final length, but the amount already recorded must not
 * disappear while it is being finalized. */
export function taskSizeSummary(task: {
  downloaded_bytes?: number
  total_bytes?: number
  is_live?: boolean
}): string {
  const downloaded = Math.max(0, Number(task.downloaded_bytes) || 0)
  const total = Math.max(0, Number(task.total_bytes) || 0)
  const totalLabel = total > 0
    ? `总大小 ${fmtBytes(total)}`
    : task.is_live ? '总大小 未知（直播）' : '总大小 未知'
  return `已下载 ${fmtBytes(downloaded)} · ${totalLabel}`
}

const ACTIVE = new Set([
  'queued', 'awaiting_confirmation', 'fetching_metadata', 'awaiting_selection',
  'checking', 'downloading', 'downloading_m3u8', 'parsing',
  'downloading_segments', 'pausing', 'paused', 'merging', 'remuxing',
])

export function taskMatchesFilter(task: Record<string, any>, filter: string): boolean {
  const status = String(task.status || '')
  if (!filter || filter === 'all') return true
  if (filter === 'running') return isRunningStatus(status) || status === 'queued'
  if (filter === 'queued') return ['queued', 'awaiting_confirmation', 'awaiting_selection'].includes(status)
  if (filter === 'paused') return status === 'paused' || status === 'pausing'
  if (filter === 'failed') return status === 'failed' || status === 'unsupported'
  if (['media', 'program', 'archive', 'other'].includes(filter)) {
    return downloadCategory(task.output_path || task.filename || task.url, task.mime_type, task.task_type) === filter
  }
  return status === filter
}

const FILTER_EMPTY_TITLES: Record<string, string> = {
  running: '没有正在进行的任务',
  queued: '队列是空的',
  paused: '没有已暂停的任务',
  done: '还没有完成的任务',
  failed: '没有失败的任务',
  media: '没有媒体任务',
  program: '没有程序任务',
  archive: '没有压缩包任务',
  other: '没有其他分类任务',
}

export function emptyTaskListCopy(filter: string, query: string, totalCount = 0): { title: string; hint: string } {
  if (query.trim()) {
    return { title: '没有匹配的任务', hint: '试试缩短关键词，或清空搜索框查看全部任务' }
  }
  if (filter && filter !== 'all') {
    return {
      title: FILTER_EMPTY_TITLES[filter] || '当前分类没有任务',
      hint: totalCount > 0 ? '可切换到“全部任务”查看其它状态' : '点击“新建”添加文件、HLS、DASH、magnet 或种子',
    }
  }
  return { title: '暂无任务', hint: '点击“新建”添加文件、HLS、DASH、magnet 或种子' }
}

export function filterAndSortTasks<T extends Record<string, any>>(
  tasks: T[],
  filter: string,
  query: string,
): T[] {
  const needle = query.trim().toLocaleLowerCase()
  return tasks.filter(task => {
    if (!taskMatchesFilter(task, filter)) return false
    if (!needle) return true
    return [task.id, task.title, task.filename, task.url, task.error_code, task.error_message]
      .some(value => String(value || '').toLocaleLowerCase().includes(needle))
  }).sort((a, b) => {
    const priority = (status: string) => ACTIVE.has(status) ? 0 : status === 'failed' || status === 'unsupported' ? 1 : status === 'canceled' ? 2 : 3
    const priorityDifference = priority(a.status) - priority(b.status)
    if (priorityDifference) return priorityDifference
    if (a.status === 'queued' && b.status === 'queued') {
      const position = (task: T) => {
        const value = Number(task.queue_position) || 0
        return value > 0 ? value : Number.MAX_SAFE_INTEGER
      }
      const positionDifference = position(a) - position(b)
      if (positionDifference) return positionDifference
    }
    const createdDifference = String(b.created_at || '').localeCompare(String(a.created_at || ''))
    return createdDifference || String(a.id).localeCompare(String(b.id))
  })
}
