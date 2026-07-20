export type FileKind = 'archive' | 'executable' | 'video' | 'audio' | 'image' | 'document' | 'code' | 'generic'

const GROUPS: Record<Exclude<FileKind, 'generic'>, Set<string>> = {
  archive: new Set(['zip', '7z', 'rar', 'tar', 'gz', 'bz2', 'xz', 'iso']),
  executable: new Set(['exe', 'msi', 'msix', 'appx', 'bat', 'cmd']),
  video: new Set(['mp4', 'mkv', 'webm', 'mov', 'avi', 'm4v', 'ts']),
  audio: new Set(['mp3', 'm4a', 'aac', 'flac', 'wav', 'ogg']),
  image: new Set(['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'svg']),
  document: new Set(['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'txt']),
  code: new Set(['js', 'ts', 'json', 'html', 'css', 'py', 'java', 'c', 'cpp']),
}

export function filePresentation(path: string, mimeType = ''): { kind: FileKind; extension: string } {
  const clean = path.split(/[?#]/, 1)[0]
  const name = clean.split(/[\\/]/).pop() || ''
  const extension = name.includes('.') ? name.split('.').pop()!.toLowerCase().slice(0, 5) : ''
  for (const [kind, values] of Object.entries(GROUPS) as [Exclude<FileKind, 'generic'>, Set<string>][]) {
    if (values.has(extension)) return { kind, extension }
  }
  if (mimeType.startsWith('video/')) return { kind: 'video', extension }
  if (mimeType.startsWith('audio/')) return { kind: 'audio', extension }
  if (mimeType.startsWith('image/')) return { kind: 'image', extension }
  return { kind: 'generic', extension }
}
