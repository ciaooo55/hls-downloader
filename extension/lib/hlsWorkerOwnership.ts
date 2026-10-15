// hls.js Worker 转封装保留这些分片字段；只按完整标识关联输入和输出，不按消息时间猜测。
export function hlsWorkerChunkKey(message: any): string | null {
  const metadata = message?.event === 'flush' ? message.data : message?.chunkMeta || message?.data?.chunkMeta
  // hls.js 1.4 的 Worker 独占一个转封装实例，不发送 instanceNo；映射本来就按 Worker 隔离。
  const instance = message?.instanceNo === undefined ? 'dedicated' : message.instanceNo
  const fields = [metadata?.level, metadata?.sn, metadata?.id, metadata?.part]
  if (message?.instanceNo !== undefined && !Number.isSafeInteger(message.instanceNo)) return null
  return fields.every(value => Number.isSafeInteger(value)) ? [instance, ...fields].join(':') : null
}

export function hlsWorkerOutputBuffers(message: any): object[] {
  if (message?.event !== 'transmuxComplete') return []
  const result = message.data?.remuxResult
  const buffers = [result?.audio?.data1, result?.audio?.data2, result?.video?.data1, result?.video?.data2,
    ...Object.values(result?.initSegment?.tracks || {}).map((track: any) => track?.initSegment)]
  return buffers.filter(value => value instanceof ArrayBuffer || ArrayBuffer.isView(value))
}
