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
})
