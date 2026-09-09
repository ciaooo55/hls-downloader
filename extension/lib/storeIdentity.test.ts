import { describe, expect, it } from 'vitest'
import { CHROMIUM_EXTENSION_ID, CHROMIUM_PUBLIC_KEY, FIREFOX_EXTENSION_ID } from './storeIdentity'

describe('3.x store identity continuity', () => {
  it('keeps the Chromium Web Store identity', () => {
    expect(CHROMIUM_EXTENSION_ID).toBe('bbdfldcjnikaemnimalegbopgaknjhla')
    expect(CHROMIUM_PUBLIC_KEY).toMatch(/^MIGfMA0G/)
  })

  it('keeps the Firefox AMO identity', () => {
    expect(FIREFOX_EXTENSION_ID).toBe('hls-downloader-store@ciaooo55.com')
  })
})
