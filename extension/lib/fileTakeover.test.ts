import { describe, expect, it } from 'vitest'
import {
  ACCELERATED_TAKEOVER_MIN_BYTES,
  canAccelerateBrowserDownload,
  clickIntentPollsForKind,
  earlyTakeoverRequiresClick,
  isOrdinaryFileKind,
  looksSingleUseAuthorizedDownload,
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

  it('accelerates normal HTTP(S) GET downloads including canResume=false, small/unknown-size, un-ranged 200, and S3 presigned URLs', () => {
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      canResume: false,
      responseHeaders: {
        'accept-ranges': 'bytes',
        'content-length': String(50 * 1024 * 1024),
      },
    })).toBe(true)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      canResume: false,
      responseHeaders: { 'content-length': String(64 * 1024) },
    })).toBe(true)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      canResume: false,
      responseHeaders: {},
      itemTotalBytes: -1,
      itemFileSize: -1,
    })).toBe(true)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      statusCode: undefined,
      responseHeaders: undefined,
    })).toBe(true)
    const s3Presigned = 'https://release-assets.githubusercontent.com/12345/archive.zip?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIA%2F20261125%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20261125T080000Z&X-Amz-Expires=300&X-Amz-Signature=deadbeef'
    expect(looksSingleUseAuthorizedDownload(s3Presigned)).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      url: s3Presigned,
      canResume: false,
      responseHeaders: { 'content-length': String(25 * 1024 * 1024) },
    })).toBe(true)
  })

  it('preserves non-GET, non-HTTP(S), completed, and non-media/file transfers', () => {
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), method: 'POST' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), url: 'blob:https://site.test/abc' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), url: 'data:application/octet-stream,abc' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), itemState: 'complete' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), kind: 'hls' })).toBe(false)
    expect(canAccelerateBrowserDownload({ ...resumableLargeGet(), statusCode: 403 })).toBe(false)
  })

  it('rejects genuine one-use no-store/OAuth/SAML transfers, cookie mismatches, and sizes below configured minimumBytes', () => {
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: {
        'accept-ranges': 'bytes',
        'content-length': String(50 * 1024 * 1024),
        'cache-control': 'private, no-store',
      },
    })).toBe(false)
    expect(looksSingleUseAuthorizedDownload('https://idp.example.com/sso/saml2?SAMLResponse=abc')).toBe(true)
    expect(looksSingleUseAuthorizedDownload('https://site.test/oauth/callback?code=auth123&state=xyz')).toBe(true)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      url: 'https://site.test/oauth/callback?code=auth123&state=xyz',
    })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      cookieParityOk: false,
    })).toBe(false)
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      minimumBytes: 100 * 1024 * 1024,
    })).toBe(false)
    // Unknown Content-Length remains eligible even when minimumBytes > 0.
    expect(canAccelerateBrowserDownload({
      ...resumableLargeGet(),
      responseHeaders: {},
      itemTotalBytes: 0,
      itemFileSize: 0,
      minimumBytes: 1 * 1024 * 1024,
    })).toBe(true)
  })
})
