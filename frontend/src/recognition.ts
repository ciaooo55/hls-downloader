export interface RecognitionResult {
  kind: 'hls' | 'page' | 'none'
  candidates: Array<{ url: string; source?: string }>
  message?: string
}

export interface RecognitionView {
  mode: 'ready' | 'choose' | 'not-found'
  message: string
}

export function recognitionView(result: RecognitionResult): RecognitionView {
  if (result.candidates.length === 1) {
    return { mode: 'ready', message: result.message || '' }
  }
  if (result.candidates.length > 1) {
    return { mode: 'choose', message: result.message || '' }
  }
  return { mode: 'not-found', message: result.message || '未发现可下载的 HLS 链接' }
}
