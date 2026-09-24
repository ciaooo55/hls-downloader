import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const popupRoot = fileURLToPath(new URL('../entrypoints/popup/', import.meta.url))
const html = readFileSync(`${popupRoot}/index.html`, 'utf8')
const main = readFileSync(`${popupRoot}/main.ts`, 'utf8')
const background = readFileSync(fileURLToPath(new URL('../entrypoints/background.ts', import.meta.url)), 'utf8')
const css = readFileSync(`${popupRoot}/style.css`, 'utf8')

describe('popup first paint', () => {
  it('keeps the 400px preferred width but clamps boot and loaded surfaces to the viewport', () => {
    for (const source of [html, css]) {
      const bodyRule = source.match(/body\s*\{([^}]+)\}/)?.[1] ?? ''
      expect(bodyRule).toMatch(/(?:^|;)\s*width:\s*400px;/)
      expect(bodyRule).toMatch(/max-width:\s*100vw;/)
    }
  })

  it('lets resource cards and branding shrink without losing action buttons', () => {
    expect(css).toMatch(/article\s*\{[^}]*min-width:\s*0;/)
    expect(css).toMatch(/\.brand\s*>\s*div\s*\{[^}]*min-width:\s*0;/)
    expect(css).toMatch(/\.brand img,\s*\.header-actions\s*\{[^}]*flex-shrink:\s*0;/)
  })

  it('ships a visible no-script/bootstrap surface instead of an empty root', () => {
    expect(html).toContain('<main class="popup-boot"')
    expect(html).toContain('正在读取当前页面')
    expect(html).not.toContain('<div id="root"></div>')
  })

  it('commits the interactive shell before restoring optional theme state', () => {
    const shell = main.indexOf('root.append(mainEl)')
    const storage = main.indexOf('void withDeadline(browser.storage.local.get(THEME_STORAGE_KEY)')
    expect(shell).toBeGreaterThan(0)
    expect(storage).toBeGreaterThan(shell)
    expect(main).toContain("dataset.popupReady = 'shell'")
    expect(main).not.toContain('const storedTheme = await')
  })

  it('renders a readable error state when asynchronous bootstrap fails', () => {
    expect(main).toContain('function renderStartupError')
    expect(main).toContain('插件界面加载失败')
    expect(main).toContain('void main().catch(renderStartupError)')
  })

  it('ignores a theme restore that is older than the last click', () => {
    // The storage read is issued after the shell is painted but resolves later,
    // with the value that was stored before any click. Without a generation
    // guard the restore re-stamps the old theme and rolls themePreference back
    // while storage already holds the new one, so the DOM and storage disagree.
    expect(main).toContain('let themeChangeGeneration = 0')
    expect(main).toContain('themeChangeGeneration += 1')
    expect(main).toContain('const themeRestoreGeneration = themeChangeGeneration')
    expect(main).toContain('if (themeRestoreGeneration !== themeChangeGeneration) return')
  })


  it('attaches a rejection handler to fire-and-forget resource and badge writes', () => {
    // saveResource/refreshTabBadge/refreshOpenTabBadges hand back the raw
    // SessionListStore.update operation (only the store's internal tail swallows
    // errors), so a bare `void` site turns a storage.session failure into an
    // unhandled rejection in the service worker, where no caller can log or
    // recover from it.
    //
    // A promise chain keeps going when the next line starts with a member access
    // or a closing delimiter, so statements are reassembled before checking.
    const statements: string[] = []
    let current = ''
    const lines = background.split(/\r?\n/)
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index].trim()
      if (!line) continue
      current = current ? `${current} ${line}` : line
      const next = (lines[index + 1] || '').trim()
      if (next.startsWith('.') || next.startsWith(')') || next.startsWith(',')) continue
      statements.push(current)
      current = ''
    }
    if (current) statements.push(current)

    const guarded = /^void (?:saveResource|refreshTabBadge|refreshOpenTabBadges)\(/
    const sites = statements.filter(statement => guarded.test(statement))
    expect(sites.length).toBeGreaterThan(0)
    for (const site of sites) {
      expect(site, `unguarded fire-and-forget call: ${site}`).toContain('.catch(')
    }
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
