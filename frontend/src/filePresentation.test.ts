import { describe, expect, it } from 'vitest'
import { filePresentation } from './filePresentation'

describe('filePresentation', () => {
  it('distinguishes executables, archives and media', () => {
    expect(filePresentation('setup.exe')).toEqual({ kind: 'executable', extension: 'exe' })
    expect(filePresentation('backup.7z')).toEqual({ kind: 'archive', extension: '7z' })
    expect(filePresentation('movie.mp4')).toEqual({ kind: 'video', extension: 'mp4' })
  })

  it('falls back to MIME type when a URL has no extension', () => {
    expect(filePresentation('https://cdn.test/download?id=1', 'audio/mpeg').kind).toBe('audio')
  })
})
