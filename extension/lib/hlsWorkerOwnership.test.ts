import { describe, expect, it } from 'vitest'
import { hlsWorkerChunkKey, hlsWorkerOutputBuffers } from './hlsWorkerOwnership'

describe('hls.js Worker media ownership', () => {
  it('correlates dedicated hls.js 1.4 worker output without an instance number', () => {
    const chunkMeta = { level: 0, sn: 12, id: 1, part: -1 }
    const key = hlsWorkerChunkKey({ cmd: 'demux', chunkMeta })
    expect(key).not.toBeNull()
    expect(hlsWorkerChunkKey({ event: 'transmuxComplete', data: { chunkMeta } })).toBe(key)
    expect(hlsWorkerChunkKey({ event: 'flush', data: chunkMeta })).toBe(key)
    expect(hlsWorkerChunkKey({ instanceNo: 0, cmd: 'demux', chunkMeta })).not.toBe(key)
    expect(hlsWorkerChunkKey({ instanceNo: null, chunkMeta })).toBeNull()
  })
  it('correlates the same chunk across demux, completion and flush, separating workers and parts', () => {
    const chunkMeta = { level: 1, sn: 42, id: 3, part: -1 }
    const input = { instanceNo: 7, cmd: 'demux', chunkMeta }
    const output = { instanceNo: 7, event: 'transmuxComplete', data: { chunkMeta } }
    expect(hlsWorkerChunkKey(input)).toEqual(hlsWorkerChunkKey(output))
    expect(hlsWorkerChunkKey({ instanceNo: 7, event: 'flush', data: chunkMeta })).toEqual(hlsWorkerChunkKey(input))
    expect(hlsWorkerChunkKey({ ...input, instanceNo: 8 })).not.toEqual(hlsWorkerChunkKey(output))
    expect(hlsWorkerChunkKey({ ...input, chunkMeta: { ...chunkMeta, part: 0 } })).not.toEqual(hlsWorkerChunkKey(output))
    expect(hlsWorkerChunkKey({ instanceNo: 7, chunkMeta: { sn: 42 } })).toBeNull()
  })
  it('tracks video, audio and initialization outputs without treating metadata as downloadable bytes', () => {
    const video = new Uint8Array(8), audio = new ArrayBuffer(4), init = new Uint8Array(2)
    const message = { event: 'transmuxComplete', data: { remuxResult: {
      video: { data1: video, data2: 'invalid' }, audio: { data1: audio },
      initSegment: { tracks: { video: { initSegment: init } } },
    } } }
    expect(hlsWorkerOutputBuffers(message)).toEqual([audio, video, init])
    expect(hlsWorkerOutputBuffers({ ...message, event: 'workerLog' })).toEqual([])
  })
})
