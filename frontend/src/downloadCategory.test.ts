import { describe, expect, it } from 'vitest'
import { downloadCategory } from './downloadCategory'

describe('download categories', () => {
  it('groups files into the four user-facing categories', () => {
    expect(downloadCategory('movie.mp4')).toBe('media')
    expect(downloadCategory('cover.webp')).toBe('media')
    expect(downloadCategory('setup.exe')).toBe('program')
    expect(downloadCategory('files.7z')).toBe('archive')
    expect(downloadCategory('manual.pdf')).toBe('other')
  })

  it('always treats HLS and DASH tasks as media', () => {
    expect(downloadCategory('download.php', 'text/plain', 'hls')).toBe('media')
    expect(downloadCategory('manifest', '', 'dash')).toBe('media')
  })
})
