import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const popupRoot = fileURLToPath(new URL('../entrypoints/popup/', import.meta.url))
const html = readFileSync(`${popupRoot}/index.html`, 'utf8')
const main = readFileSync(`${popupRoot}/main.ts`, 'utf8')

describe('popup first paint', () => {
  it('ships a visible no-script/bootstrap surface instead of an empty root', () => {
    expect(html).toContain('<main class="popup-boot"')
    expect(html).toContain('正在读取当前页面')
    expect(html).not.toContain('<div id="root"></div>')
  })

  it('commits the interactive shell before awaiting stored theme state', () => {
    const shell = main.indexOf('root.append(mainEl)')
    const storage = main.indexOf('const storedTheme = await')
    expect(shell).toBeGreaterThan(0)
    expect(storage).toBeGreaterThan(shell)
    expect(main).toContain("dataset.popupReady = 'shell'")
  })

  it('renders a readable error state when asynchronous bootstrap fails', () => {
    expect(main).toContain('function renderStartupError')
    expect(main).toContain('插件界面加载失败')
    expect(main).toContain('void main().catch(renderStartupError)')
  })

  it('offers a real current-page rescan with distinct loading and empty states', () => {
    expect(main).toContain("browser.tabs.sendMessage(tab.id, { type: 'rescan-media' })")
    expect(main).toContain("resourceState: 'loading' | 'ready' | 'scanning' | 'error'")
    expect(main).toContain('\\u91cd\\u65b0\\u8bc6\\u522b')
    expect(main).toContain('\\u5f53\\u524d\\u9875\\u9762\\u8fd8\\u6ca1\\u6709\\u53ef\\u4e0b\\u8f7d\\u8d44\\u6e90')
    expect(main).toContain("candidate.active && /^https?:\\/\\//i.test(candidate.url || '')")
    expect(main).toContain('actionCol.append(button, castButton, pushButton)')
    expect(main).toContain('resourceSuffix(item)')
  })
})
