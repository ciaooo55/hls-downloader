import { describe, expect, it } from 'vitest'
import {
  ACCELERATED_TAKEOVER_MIN_BYTES,
  canAccelerateBrowserDownload,
  clickIntentPollsForKind,
  earlyTakeoverRequiresClick,
  isOrdinaryFileKind,
  ordinaryFileResponseIdentified,
} from './fileTakeover'

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

  it('takes server attachments without a click while leaving media previews gated', () => {
    for (const kind of ['media', 'hls', 'dash'] as const) {
      expect(earlyTakeoverRequiresClick(kind, 'attachment; filename="export.dat"')).toBe(false)
      expect(earlyTakeoverRequiresClick(kind, 'inline; filename="export.dat"')).toBe(true)
      expect(earlyTakeoverRequiresClick(kind)).toBe(true)
    }
  })
})

describe('accelerated multi-connection takeover eligibility', () => {
  const resumableLargeGet = () => ({
    kind: 'file' as const,
    url: 'https://cdn.test/releases/app-installer.iso',
    method: 'GET',
    statusCode: 200,
    itemState: 'in_progress',
    responseHeaders: {
      'accept-ranges': 'bytes',
      'content-length': String(50 * 1024 * 1024),
      etag: '"iso-v1"',
    },
    cookieParityOk: true,
  })

  it('accelerates large resumable HTTP(S) GET file and media transfers with validators', () => {
    expect(ACCELERATED_TAKEOVER_MIN_BYTES).toBe(1 * 1024 * 1024)
    expect(canAccelerateBrowserDownload(resumableLargeGet())).toBe(true)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      kind: 'media',
      url: 'http://mirror.test/movie.mp4',
      // Chrome reports canResume: false while a download is actively in_progress;
      // a strong validator still proves range resumability.
      canResume: false,
      responseHeaders: {
        'Accept-Ranges': 'BYTES',
        'Content-Length': String(2 * 1024 * 1024),
        'Last-Modified': 'Wed, 21 Oct 2026 07:28:00 GMT',
      },
    })).toBe(true)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      statusCode: 206,
      responseHeaders: {
        'accept-ranges': 'bytes',
        'content-range': 'bytes 0-1048575/10485760',
        etag: '"range-v1"',
      },
    })).toBe(true)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: {
        'accept-ranges': 'bytes',
        etag: '"item-size-fallback"',
      },
      itemTotalBytes: 5 * 1024 * 1024,
    })).toBe(true)
  })

  it('preserves small, unknown-size, non-GET, non-HTTP, completed, and non-range transfers', () => {
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: { 'accept-ranges': 'bytes', 'content-length': String(1024 * 1024 - 1), etag: '"small"' },
    })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: { 'accept-ranges': 'bytes', etag: '"unknown-size"' },
      itemTotalBytes: -1,
      itemFileSize: -1,
    })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), method: 'POST' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), url: 'blob:https://site.test/abc' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), url: 'data:application/octet-stream,abc' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), itemState: 'complete' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), kind: 'hls' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), responseHeaders: undefined })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: { 'content-length': String(50 * 1024 * 1024), etag: '"no-range"' },
    })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: { 'accept-ranges': 'none', 'content-length': String(50 * 1024 * 1024), etag: '"none-range"' },
    })).toBe(false)
  })

  it('rejects likely one-use transfers, cookie mismatches, and sizes below configured minimumBytes', () => {
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: {
        'accept-ranges': 'bytes',
        'content-length': String(50 * 1024 * 1024),
        'cache-control': 'private, no-store',
      },
    })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      canResume: false,
      responseHeaders: {
        'accept-ranges': 'bytes',
        'content-length': String(50 * 1024 * 1024),
      },
    })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      cookieParityOk: false,
    })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      minimumBytes: 100 * 1024 * 1024,
    })).toBe(false)
  })
})
