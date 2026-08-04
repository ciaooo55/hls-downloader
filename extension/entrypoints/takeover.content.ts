import { browser } from 'wxt/browser'
import { isLikelyDownloadControl, resolveDownloadTarget, shouldTrackDownloadIntent } from '../lib/clickIntent'

export default defineContentScript({
  matches: ['<all_urls>'],
  allFrames: true,
  runAt: 'document_start',
  main() {
    let lastIntent = { key: '', at: 0 }
    const recordIntent = (event: MouseEvent) => {
      if (!event.isTrusted || event.button !== 0) return
      const anchor = event.composedPath()
        .find(value => value instanceof HTMLAnchorElement) as HTMLAnchorElement | undefined
      const control = event.composedPath().find(value => value instanceof HTMLElement
        && value.matches('button, input[type="button"], input[type="submit"], [role="button"]')) as HTMLElement | undefined
      if (!anchor?.href && !control) return
      const rawHref = anchor?.getAttribute('href')?.trim() || ''
      const directHref = rawHref && !rawHref.startsWith('#') && !/^javascript:/i.test(rawHref) ? anchor?.href || '' : ''
      const rawDownloadHref = anchor?.getAttribute('data-download-url')
        || control?.getAttribute('data-download-url')
        || ''
      const rawHintedHref = anchor?.getAttribute('data-url')
        || anchor?.getAttribute('data-href')
        || control?.getAttribute('data-url')
        || control?.getAttribute('data-href')
        || ''
      const downloadHref = resolveDownloadTarget(rawDownloadHref, location.href)
      const hintedHref = resolveDownloadTarget(rawHintedHref, location.href)
      const downloadHint = isLikelyDownloadControl([
        anchor?.textContent,
        anchor?.getAttribute('aria-label'),
        anchor?.getAttribute('title'),
        anchor?.getAttribute('name'),
        anchor?.id,
        anchor?.className,
        anchor?.getAttribute('data-testid'),
        anchor?.hasAttribute('download') ? 'download' : '',
        control?.textContent,
        control?.getAttribute('aria-label'),
        control?.getAttribute('title'),
        control?.getAttribute('name'),
        control?.getAttribute('value'),
        control?.id,
        control?.className,
        control?.getAttribute('data-testid'),
      ])
      const explicitDownloadTarget = Boolean(anchor?.hasAttribute('download') || downloadHref)
      if (!shouldTrackDownloadIntent({
        directHref,
        hintedHref: downloadHref || hintedHref,
        ctrlForce: event.ctrlKey,
        explicitDownloadTarget,
        hints: [downloadHint ? 'download' : ''],
      })) return
      const href = downloadHref || directHref || hintedHref
      const intentKey = [href, location.href, event.altKey ? 1 : 0, event.ctrlKey ? 1 : 0, downloadHint ? 1 : 0].join('|')
      const now = Date.now()
      // A pointerdown followed by click describes one user action. Keeping two
      // entries is dangerous: the first can be consumed by the intended file
      // while the second remains eligible for an unrelated download in the
      // same tab. Keyboard activation has no pointerdown and still reaches the
      // click listener normally.
      if (lastIntent.key === intentKey && now - lastIntent.at < 1_500) return
      lastIntent = { key: intentKey, at: now }
      void browser.runtime.sendMessage({
        type: 'click-intent',
        href,
        pageUrl: location.href,
        altBypass: event.altKey,
        ctrlForce: event.ctrlKey,
        generic: !href,
        opensNewTab: anchor?.target.toLowerCase() === '_blank',
        controlHint: downloadHint,
      })
    }
    // Many download controls start navigation from pointerdown or tear down the
    // document before their click handler returns. Record strong intent at the
    // earliest trusted phase so onHeadersReceived can present immediately.
    window.addEventListener('pointerdown', recordIntent, true)
    window.addEventListener('click', recordIntent, true)
  },
})
