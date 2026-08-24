import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const extensionRoot = fileURLToPath(new URL('../', import.meta.url))
const content = readFileSync(`${extensionRoot}/entrypoints/content.ts`, 'utf8')
const popup = readFileSync(`${extensionRoot}/entrypoints/popup/main.ts`, 'utf8')
const background = readFileSync(`${extensionRoot}/entrypoints/background.ts`, 'utf8')

describe('explicit extension download flow', () => {
  it('does not ask for a second confirmation after the user clicks Download', () => {
    expect(content).toContain("type: 'download-now'")
    expect(popup).toContain("type: 'download-now'")
    expect(background).toContain("message?.type === 'download-now'")
    expect(content).not.toContain("setSendState(resource, button, '等待确认'")
    expect(popup).not.toContain("type: 'handoff-status'")
    expect(popup).not.toContain('window.setInterval')
    expect(background).toContain('void downloadNow(resource, undefined, { allowUnverified: true })')
    expect(background).toContain("resource.evidence.includes('text_selection')")
  })

  it('keeps automatic browser takeover on the confirmation route', () => {
    expect(background).toContain("message?.type === 'download' || message?.type === 'offer'")
    expect(background).toContain('const request = fromPage || message.type === \'offer\' ? offer(resource) : downloadNow(resource)')
  })

  it('bounds popup and in-page waits instead of leaving controls stuck', () => {
    expect(content).toContain("10_000, '下载器响应超时，请重试'")
    expect(popup).toContain("10_000,")
    expect(popup).toContain("1_800")
  })
})
