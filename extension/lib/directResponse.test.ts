import { describe, expect, it } from 'vitest'
import { isEarlyDirectDownloadResponse } from './directResponse'

const resource = (overrides: Record<string, unknown> = {}) => ({
  url: 'https://cdn.test/file.mp4',
  kind: 'media' as const,
  mimeType: 'video/mp4',
  ...overrides,
})

describe('early Chromium direct-download response detection', () => {
  it('accepts an attachment navigation before downloads.onCreated', () => {
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: 200 },
      { disposition: 'attachment; filename="video.mp4"', resource: resource() },
    )).toBe(true)
  })

  it('does not pre-offer a response the server explicitly marked inline', () => {
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: 200 },
      {
        disposition: 'inline; filename="manual.pdf"',
        resource: resource({
          url: 'https://docs.test/manual',
          kind: 'file',
          mimeType: 'application/pdf',
          filename: 'manual.pdf',
        }),
      },
    )).toBe(false)
  })

  it('accepts a direct media navigation without Content-Disposition', () => {
    expect(isEarlyDirectDownloadResponse(
      { type: 'sub_frame', method: 'GET', statusCode: 206 },
      { disposition: '', resource: resource() },
    )).toBe(true)
  })

  it('rejects successful statuses that cannot carry downloadable content', () => {
    for (const statusCode of [204, 205]) {
      expect(isEarlyDirectDownloadResponse(
        { type: 'main_frame', method: 'GET', statusCode },
        { disposition: 'attachment; filename="empty.zip"', resource: resource({ kind: 'file' }) },
      )).toBe(false)
    }
  })

  it('accepts a direct installer/archive navigation without Content-Disposition', () => {
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: 200 },
      {
        disposition: '',
        resource: {
          url: 'https://mirror.test/ubuntu-24.04.iso',
          kind: 'file',
          mimeType: 'application/octet-stream',
        },
      },
    )).toBe(true)
  })

  it('does not pre-offer XHR, HTML, failed, or statusless responses', () => {
    expect(isEarlyDirectDownloadResponse(
      { type: 'xmlhttprequest', method: 'GET', statusCode: 200 },
      { disposition: 'attachment', resource: resource() },
    )).toBe(false)
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: 200 },
      { disposition: '', resource: resource({ kind: 'file', mimeType: 'text/html', url: 'https://site.test/page' }) },
    )).toBe(false)
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: 403 },
      { disposition: 'attachment', resource: resource() },
    )).toBe(false)
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET' },
      { disposition: 'attachment', resource: resource() },
    )).toBe(false)
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: Number.NaN },
      { disposition: 'attachment', resource: resource() },
    )).toBe(false)
  })

  it('accepts extensionless zip/pdf responses that already classified as files', () => {
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: 200 },
      {
        disposition: '',
        resource: {
          url: 'https://cdn.test/get?id=1',
          kind: 'file',
          mimeType: 'application/zip',
          filename: '',
        },
      },
    )).toBe(true)
    expect(isEarlyDirectDownloadResponse(
      { type: 'main_frame', method: 'GET', statusCode: 200 },
      {
        disposition: '',
        resource: {
          url: 'https://cdn.test/get?id=1',
          kind: 'file',
          mimeType: 'application/octet-stream',
          filename: 'report.pdf',
        },
      },
    )).toBe(true)
  })
})
