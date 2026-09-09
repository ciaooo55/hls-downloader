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

  it('accepts a direct media navigation without Content-Disposition', () => {
    expect(isEarlyDirectDownloadResponse(
      { type: 'sub_frame', method: 'GET', statusCode: 206 },
      { disposition: '', resource: resource() },
    )).toBe(true)
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

  it('does not pre-offer XHR, HTML, or failed responses', () => {
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
