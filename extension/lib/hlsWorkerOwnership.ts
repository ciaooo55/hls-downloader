// hls.js Worker 转封装保留这些分片字段；只按完整标识关联输入和输出，不按消息时间猜测。
export function hlsWorkerChunkKey(message: any): string | null {
  const metadata = message?.event === 'flush' ? message.data : message?.chunkMeta || message?.data?.chunkMeta
  const fields = [message?.instanceNo, metadata?.level, metadata?.sn, metadata?.id, metadata?.part]
  return fields.every(value => Number.isSafeInteger(value)) ? fields.join(':') : null
}

export function hlsWorkerOutputBuffers(message: any): object[] {
  if (message?.event !== 'transmuxComplete') return []
  const result = message.data?.remuxResult
  const buffers = [result?.audio?.data1, result?.audio?.data2, result?.video?.data1, result?.video?.data2,
    ...Object.values(result?.initSegment?.tracks || {}).map((track: any) => track?.initSegment)]
  return buffers.filter(value => value instanceof ArrayBuffer || ArrayBuffer.isView(value))
}
