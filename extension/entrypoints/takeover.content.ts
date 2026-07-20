import { browser } from 'wxt/browser'

export default defineContentScript({
  matches: ['<all_urls>'],
  runAt: 'document_start',
  main() {
    window.addEventListener('click', event => {
      if (!event.isTrusted || event.button !== 0) return
      const anchor = event.composedPath()
        .find(value => value instanceof HTMLAnchorElement) as HTMLAnchorElement | undefined
      const control = event.composedPath().find(value => value instanceof HTMLElement
        && value.matches('button, input[type="button"], input[type="submit"], [role="button"]'))
      if (!anchor?.href && !control) return
      const rawHref = anchor?.getAttribute('href')?.trim() || ''
      void browser.runtime.sendMessage({
        type: 'click-intent',
        href: anchor?.href || '',
        pageUrl: location.href,
        altBypass: event.altKey,
        ctrlForce: event.ctrlKey,
        generic: !anchor || !rawHref || rawHref.startsWith('#') || /^javascript:/i.test(rawHref),
      })
    }, true)
  },
})
