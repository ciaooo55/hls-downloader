import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const extensionRoot = fileURLToPath(new URL('../', import.meta.url))
const hooks = readFileSync(`${extensionRoot}/entrypoints/hooks.content.ts`, 'utf8')

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
