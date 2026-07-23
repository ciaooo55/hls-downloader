import { describe, expect, it } from 'vitest'
import { recognitionView } from './recognition'

describe('recognitionView', () => {
  it('starts one confirmed playlist directly', () => {
    expect(recognitionView({ kind: 'hls', candidates: [{ url: 'https://x/a' }] }).mode).toBe('ready')
  })

  it('asks the user to choose between multiple candidates', () => {
    expect(recognitionView({ kind: 'page', candidates: [{ url: 'a' }, { url: 'b' }] }).mode).toBe('choose')
  })

  it('shows browser extension guidance when static recognition finds nothing', () => {
    const view = recognitionView({ kind: 'none', candidates: [], message: '请使用浏览器插件' })
    expect(view.mode).toBe('not-found')
    expect(view.message).toContain('浏览器插件')
  })
})
