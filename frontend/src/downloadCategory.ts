import { filePresentation } from './filePresentation'

export type DownloadCategory = 'media' | 'program' | 'archive' | 'other'

export function downloadCategory(path: string, mimeType = '', taskType = ''): DownloadCategory {
  if (taskType === 'hls' || taskType === 'dash') return 'media'
  const { kind } = filePresentation(path, mimeType)
  if (kind === 'video' || kind === 'audio' || kind === 'image') return 'media'
  if (kind === 'executable') return 'program'
  if (kind === 'archive') return 'archive'
  return 'other'
}

export const DOWNLOAD_CATEGORY_LABELS: Record<DownloadCategory, string> = {
  media: '媒体',
  program: '程序',
  archive: '压缩包',
  other: '其他',
}
