import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

/*
 * 浏览器下载栏引用计数与 XHR 监听器的守卫
 * ------------------------------------
 * 这两处都在 service worker / MAIN world 里，没有可注入的纯函数，也没有 jsdom 环境，
 * 因此沿用本仓库既有的源码文本守卫形式（与 `explicitDownload.test.ts` 同路），
 * 钉住的是"不变量"，不是排版。
 *
 * 各自对应的真实缺陷：
 * 1. `revealBrowserDownload()` 在引用计数为 0 时仍然恢复下载栏。takeover 删除浏览器
 *    下载项要经过 conceal → removeBrowserDownload → reveal 的窗口期；此间任何一条 unrelated
 *    的 onCreated 走到 `finally: revealBrowserDownload()`，或 nativeBridge 的
 *    `disconnected()`（单次超时重试也会触发）都会把计数打到 0 并立即恢复下载栏，
 *    即将被移除的项就闪回给用户。判据：计数为 0 必须直接返回。
 * 2. `XMLHttpRequest.prototype.open` 每次调用都 addEventListener('load', …)。闭包互不相同、
 *    从不移除，复用同一个 XHR 的轮询播放器最终会为一次响应触发 N 次上报。判据：按对象
 *    只注册一次，并且请求 URL 由 WeakMap 记住而不是由 open 的闭包捕获。
 */

const extensionRoot = fileURLToPath(new URL('../', import.meta.url))
const background = readFileSync(`${extensionRoot}/entrypoints/background.ts`, 'utf8')
const hooks = readFileSync(`${extensionRoot}/entrypoints/hooks.content.ts`, 'utf8')

describe('browser download shelf reference counting', () => {
  it('refuses to reveal the shelf when nothing is concealed', () => {
    const body = background.slice(
      background.indexOf('function revealBrowserDownload'),
      background.indexOf('function successfulChainForResource'),
    )
    expect(body).toContain('if (concealedDownloadCount <= 0) return')
    // 配对的 conceal 必须仍然递增并真正隐藏，否则守卫会把下载栏永久藏起来。
    const conceal = background.slice(
      background.indexOf('function concealBrowserDownload'),
      background.indexOf('function revealBrowserDownload'),
    )
    expect(conceal).toContain('concealedDownloadCount += 1')
    expect(conceal).toContain('setBrowserDownloadUi(false)')
  })
})

describe('xhr load listener registration', () => {
  it('registers the load listener once per XHR object', () => {
    const patch = hooks.slice(hooks.indexOf('const open = XMLHttpRequest.prototype.open'))
    expect(patch).toContain('if (!instrumentedXhrs.has(this))')
    expect(patch).toContain('instrumentedXhrs.add(this)')
    // 每个对象只注册一次，因此 open 里 addEventListener 的调用点必须只有一个。
    const registrations = patch.slice(0, patch.indexOf('if (async === undefined)'))
      .match(/this\.addEventListener\('load'/g) ?? []
    expect(registrations).toHaveLength(1)
  })

  it('resolves the requested URL from per-object state, not from the open() closure', () => {
    const patch = hooks.slice(hooks.indexOf('const open = XMLHttpRequest.prototype.open'))
    expect(patch).toContain('const xhrRequestedUrls = new WeakMap<XMLHttpRequest, string>()')
    expect(patch).toContain('xhrRequestedUrls.set(this, String(url))')
    expect(patch).toContain('this.responseURL || xhrRequestedUrls.get(this)')
    expect(patch).not.toContain('this.responseURL || String(url)')
  })
})
