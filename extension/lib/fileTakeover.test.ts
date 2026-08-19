import { describe, expect, it } from 'vitest'
import { clickIntentPollsForKind, earlyTakeoverRequiresClick, isOrdinaryFileKind, ordinaryFileResponseIdentified } from './fileTakeover'

describe('ordinary file takeover', () => {
  it('does not wait for a click before offering zip/exe/pdf navigations', () => {
    expect(isOrdinaryFileKind('file')).toBe(true)
    expect(isOrdinaryFileKind('magnet')).toBe(true)
    expect(isOrdinaryFileKind('media')).toBe(false)
    expect(earlyTakeoverRequiresClick('file')).toBe(false)
    expect(earlyTakeoverRequiresClick('media')).toBe(true)
    expect(earlyTakeoverRequiresClick('hls')).toBe(true)
    expect(clickIntentPollsForKind('file')).toBe(4)
    expect(clickIntentPollsForKind('media')).toBe(12)
  })

  it('accepts classified files by URL, filename or concrete MIME', () => {
    expect(ordinaryFileResponseIdentified({
      kind: 'file', url: 'https://mirror.test/app.zip', mimeType: '',
    })).toBe(true)
    expect(ordinaryFileResponseIdentified({
      kind: 'file', url: 'https://cdn.test/get?id=1', mimeType: 'application/zip',
    })).toBe(true)
    expect(ordinaryFileResponseIdentified({
      kind: 'file', url: 'https://cdn.test/get?id=1', filename: 'setup.exe', mimeType: 'application/octet-stream',
    })).toBe(true)
    expect(ordinaryFileResponseIdentified({
      kind: 'file', url: 'https://site.test/page', mimeType: 'text/html',
    })).toBe(false)
    expect(ordinaryFileResponseIdentified({
      kind: 'media', url: 'https://cdn.test/movie.mp4', mimeType: 'video/mp4',
    })).toBe(false)
  })
})
