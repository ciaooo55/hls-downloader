import { describe, expect, it } from 'vitest'
import { redactUrlForDiagnostics } from './diagnosticUrl'

describe('redactUrlForDiagnostics', () => {
  it('keeps the resource path and parameter names without their values', () => {
    const value = redactUrlForDiagnostics(
      'https://user:pass@cdn.test/video.mp4?s=secret&e=123&s=rotated#player',
    )
    expect(value).toBe('https://cdn.test/video.mp4?s=%3Credacted%3E&e=%3Credacted%3E')
    expect(value).not.toContain('secret')
    expect(value).not.toContain('user')
    expect(value).not.toContain('pass')
    expect(value).not.toContain('player')
  })

  it('does not expose non-http payloads or malformed input', () => {
    expect(redactUrlForDiagnostics('magnet:?xt=urn:btih:secret')).toBe('magnet:?<redacted>')
    expect(redactUrlForDiagnostics('not a url')).toBe('<invalid-or-non-url>')
  })
})
