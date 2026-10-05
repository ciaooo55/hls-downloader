import { describe, expect, it } from 'vitest'
import { hlsWorkerChunkKey, hlsWorkerOutputBuffers } from './hlsWorkerOwnership'

describe('hls.js Worker media ownership', () => {
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
