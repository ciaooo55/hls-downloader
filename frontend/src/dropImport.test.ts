import { describe, expect, it } from 'vitest'
import { classifyDroppedFilename, isInternalDropUrl, planDroppedPayload } from './dropImport'

describe('classifyDroppedFilename', () => {
  it('accepts torrent and explorer link files only', () => {
    expect(classifyDroppedFilename('C:\\seed\\a.torrent')).toBe('torrent')
    expect(classifyDroppedFilename('movie.m3u8')).toBe('link')
    expect(classifyDroppedFilename('files.html')).toBe('link')
    expect(classifyDroppedFilename('ubuntu.meta4')).toBe('link')
    expect(classifyDroppedFilename('pkg.metalink')).toBe('link')
    expect(classifyDroppedFilename('notes.txt')).toBeNull()
    expect(classifyDroppedFilename('photo.jpg')).toBeNull()
  })
})

describe('planDroppedPayload', () => {
  it('opens one remote URL in the existing recognize dialog', () => {
    expect(planDroppedPayload({ text: 'https://cdn.example.test/a.mp4' })).toEqual({
      kind: 'recognize',
      url: 'https://cdn.example.test/a.mp4',
    })
  })

  it('sends multiple remote URLs to batch add', () => {
    const plan = planDroppedPayload({ text: 'https://cdn.example.test/a.mp4\nhttps://cdn.example.test/b.zip' })
    expect(plan.kind).toBe('batch')
    if (plan.kind === 'batch') expect(plan.urls).toHaveLength(2)
  })

  it('ignores local app file URLs dragged from the task list', () => {
    expect(planDroppedPayload({ text: 'http://127.0.0.1:8765/api/tasks/abc/file?token=1' })).toEqual({ kind: 'none' })
    expect(isInternalDropUrl('http://127.0.0.1:8765/api/tasks/abc/file')).toBe(true)
  })

  it('prefers dropped import files over surrounding text', () => {
    const plan = planDroppedPayload({
      text: 'https://cdn.example.test/a.mp4',
      files: [{ name: 'seed.torrent', path: 'C:\\seed\\seed.torrent' }],
    })
    expect(plan).toEqual({
      kind: 'files',
      items: [{ kind: 'torrent', name: 'C:\\seed\\seed.torrent', path: 'C:\\seed\\seed.torrent' }],
    })
  })
})

