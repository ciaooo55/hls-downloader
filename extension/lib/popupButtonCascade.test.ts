import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

/*
 * Popup 按钮级联守卫
 * ------------------
 * `entrypoints/popup/main.ts` 在运行时把 `THEME_TOKENS_CSS + THEME_BASE_CSS`
 * append 到 `document.head` **末尾**；而 `style.css` 是构建产物里的一个 `<link>`。
 * 于是同特异性下，theme.ts 的通用按钮规则是"后来者胜"，会整块盖掉 style.css 里
 * 只写单类的变体规则。
 *
 * 这不是假想：`.push-button` / `.cast-button`（TVBox 推送、投屏）此前只写了单类，
 * 用 headless Chromium 按真实注入顺序实测，两个按钮渲染成
 * `rgb(43,48,53)` + `rgb(244,245,246)` —— 和普通 `.hlsd-button` 完全同色，
 * 紫色底、紫色 ink 全被盖掉，紫色语义提示整个失效；只有把注入顺序反过来才恢复预期。
 *
 * 修法：在 popup 里与 `.hlsd-button` 并用的变体一律写成 `.hlsd-button.push-button`
 * 这种带基类前缀的形式（(0,2,0)），任何注入顺序下都胜出。下面两条断言把这件事钉住：
 * 一条钉具体按钮，一条钉通用不变量。
 */

const extensionRoot = fileURLToPath(new URL('../', import.meta.url))
const popupCss = readFileSync(`${extensionRoot}/entrypoints/popup/style.css`, 'utf8')
const theme = readFileSync(`${extensionRoot}/lib/theme.ts`, 'utf8')

/** Split a sheet into `selector -> declarations`, mirroring how the cascade merges them. */
function rules(css: string): Map<string, string> {
  const out = new Map<string, string>()
  const sheet = css.replace(/\/\*[\s\S]*?\*\//g, ' ')
  for (const match of sheet.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    for (const selector of match[1].split(',')) {
      const key = selector.trim()
      if (!key) continue
      out.set(key, `${out.get(key) ?? ''} ${match[2]}`)
    }
  }
  return out
}

describe('popup button cascade', () => {
  it('keeps the purple push/cast variants specific enough to survive injection order', () => {
    // TVBox 推送 / 投屏：紫色淡底 + purple ink 是"推送到电视"的色觉提示，
    // 被通用按钮规则盖掉后两个按钮会退化成和"下载"一样的灰底。
    for (const variant of ['push-button', 'cast-button']) {
      const rest = rules(popupCss).get(`.hlsd-button.${variant}`)
      const hover = rules(popupCss).get(`.hlsd-button.${variant}:hover:not(:disabled)`)
      expect(rest, `${variant} rest rule`).toBeDefined()
      expect(hover, `${variant} hover rule`).toBeDefined()
      expect(rest).toMatch(/background:\s*color-mix\(/)
      expect(rest).toMatch(/color:\s*var\(--purple-ink\)/)
      expect(hover).toMatch(/background:\s*var\(--purple\)/)
      expect(hover).toMatch(/color:\s*var\(--on-primary\)/)
    }
  })

  it('never re-declares a `.hlsd-button` property from a single-class selector', () => {
    // 通用不变量：popup 里任何碰到 `.hlsd-button` 的选择器，都必须至少再带一个类，
    // 否则它和 theme.ts 的同名通用规则特异性相同，胜负只取决于注入顺序。
    // 现有的 `.controls .hlsd-button` / `.article-actions .hlsd-button` /
    // `.header-actions .hlsd-button.subtle` 都满足这条。
    for (const selector of rules(popupCss).keys()) {
      if (!selector.includes('.hlsd-button')) continue
      const classes = selector.match(/\.[\w-]+/g) ?? []
      expect(classes.length, `${selector} 需要再带一个类来抬高特异性`).toBeGreaterThan(1)
    }
  })

  it('gives every popup button a keyboard focus ring', () => {
    // 5 个手写按钮（update-notice / scan / empty-retry / quality-trigger / quality-menu）
    // 此前没有 :focus-visible，键盘用户完全看不到焦点在哪。共享 `.hlsd-button` 自带焦点环，
    // 其余按钮由这条通用规则兜住；实底按钮额外多留 2px 间隙，否则圆环贴着同色填充分不开。
    expect(popupCss).toContain('button:focus-visible { outline: 2px solid var(--primary); outline-offset: 1px; }')
    expect(rules(popupCss).get('.update-notice button:focus-visible')).toContain('outline-offset: 2px')
  })

  it('lifts the shared button on hover like the in-page overlay and Compose workbench do', () => {
    expect(theme).toContain('.hlsd-button:hover:not(:disabled){background:color-mix(in srgb,var(--primary) 12%,var(--surface-3));transform:translateY(-1px)}')
    // 按压反馈不能被悬吞掉：两条同特异性，active 必须声明在 hover 之后。
    const hoverAt = theme.indexOf('.hlsd-button:hover:not(:disabled)')
    const activeAt = theme.indexOf('.hlsd-button:active:not(:disabled)')
    expect(activeAt).toBeGreaterThan(hoverAt)
  })
})
