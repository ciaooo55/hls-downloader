import { browser } from 'wxt/browser'
import { isLikelyDownloadControl } from '../lib/clickIntent'

export default defineContentScript({
  matches: ['<all_urls>'],
  runAt: 'document_start',
  main() {
    window.addEventListener('click', event => {
      if (!event.isTrusted || event.button !== 0) return
      const anchor = event.composedPath()
        .find(value => value instanceof HTMLAnchorElement) as HTMLAnchorElement | undefined
      const control = event.composedPath().find(value => value instanceof HTMLElement
        && value.matches('button, input[type="button"], input[type="submit"], [role="button"]')) as HTMLElement | undefined
      if (!anchor?.href && !control) return
      const rawHref = anchor?.getAttribute('href')?.trim() || ''
      const directHref = rawHref && !rawHref.startsWith('#') && !/^javascript:/i.test(rawHref) ? anchor?.href || '' : ''
      const hintedHref = control?.getAttribute('data-download-url')
        || control?.getAttribute('data-url')
        || control?.getAttribute('data-href')
        || ''
      const downloadControl = Boolean(control && isLikelyDownloadControl([
        control.textContent,
        control.getAttribute('aria-label'),
        control.getAttribute('title'),
        control.getAttribute('name'),
        control.getAttribute('value'),
        control.id,
        control.className,
        control.getAttribute('data-testid'),
      ]))
      if (!directHref && !hintedHref && !downloadControl && !event.ctrlKey) return
      let href = directHref
      if (!href && hintedHref) {
        try { href = new URL(hintedHref, location.href).href } catch {}
      }
      void browser.runtime.sendMessage({
        type: 'click-intent',
        href,
        pageUrl: location.href,
        altBypass: event.altKey,
        ctrlForce: event.ctrlKey,
        generic: !href,
      })
    }, true)
  },
})
