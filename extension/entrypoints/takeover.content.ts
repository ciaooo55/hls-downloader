import { browser } from 'wxt/browser'
import {
  isLikelyDownloadControl,
  linkOpensNewTab,
  resolveClickedLinkHref,
  resolveDownloadTarget,
  resolveFormDownloadUrl,
  shouldTrackDownloadIntent,
} from '../lib/clickIntent'

function isHtmlLink(value: EventTarget | undefined): value is HTMLAnchorElement | HTMLAreaElement {
  return value instanceof HTMLAnchorElement || value instanceof HTMLAreaElement
}

function isSvgLink(value: EventTarget | undefined): value is SVGAElement {
  return typeof SVGAElement !== 'undefined' && value instanceof SVGAElement
}

function attribute(element: Element | undefined, name: string): string {
  return element?.getAttribute(name)?.trim() || ''
}

export default defineContentScript({
  matches: ['<all_urls>'],
  allFrames: true,
  runAt: 'document_start',
  main() {
    let lastIntent = { key: '', at: 0 }
    const recordIntent = (event: MouseEvent) => {
      if (!event.isTrusted || event.button !== 0) return
      const path = event.composedPath()
      const link = path.find(value => isHtmlLink(value) || isSvgLink(value)) as
        | HTMLAnchorElement
        | HTMLAreaElement
        | SVGAElement
        | undefined
      const control = path.find(value => value instanceof HTMLElement
        && value.matches('button, input[type="button"], input[type="submit"], [role="button"], [role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"], [data-download-url], [data-file-url], [data-export-url]')) as HTMLElement | undefined
      if (!link && !control) return
      const htmlLink = isHtmlLink(link) ? link : undefined
      const svgLink = isSvgLink(link) ? link : undefined
      const directHref = resolveClickedLinkHref({
        htmlHref: htmlLink?.href || '',
        htmlHrefAttribute: htmlLink ? attribute(htmlLink, 'href') : '',
        svgHrefAttribute: svgLink ? attribute(svgLink, 'href') : '',
        svgXlinkHref: svgLink ? attribute(svgLink, 'xlink:href') : '',
        svgBaseVal: svgLink?.href?.baseVal || '',
        baseUrl: location.href,
      })
      const rawDownloadHref = attribute(htmlLink, 'data-download-url')
        || attribute(control, 'data-download-url')
        || attribute(htmlLink, 'data-file-url')
        || attribute(control, 'data-file-url')
        || attribute(htmlLink, 'data-export-url')
        || attribute(control, 'data-export-url')
        || ''
      const rawHintedHref = attribute(htmlLink, 'data-url')
        || attribute(htmlLink, 'data-href')
        || attribute(control, 'data-url')
        || attribute(control, 'data-href')
        || ''
      const formOwner = (control && 'form' in control ? (control as HTMLButtonElement | HTMLInputElement).form : null)
        || control?.closest('form')
        || htmlLink?.closest('form')
        || null
      const formActionHref = resolveFormDownloadUrl(
        formOwner?.getAttribute('action') || '',
        attribute(control, 'formaction'),
        location.href,
      )
      const downloadHref = resolveDownloadTarget(rawDownloadHref, location.href)
      const hintedHref = resolveDownloadTarget(rawHintedHref, location.href) || formActionHref
      const controlHints = [
        htmlLink?.textContent,
        htmlLink?.getAttribute('aria-label'),
        htmlLink?.getAttribute('title'),
        htmlLink?.getAttribute('name'),
        htmlLink?.id,
        htmlLink instanceof HTMLAnchorElement || htmlLink instanceof HTMLAreaElement ? htmlLink.className : '',
        htmlLink?.getAttribute('data-testid'),
        htmlLink?.hasAttribute('download') ? 'download' : '',
        svgLink?.textContent,
        svgLink?.getAttribute('aria-label'),
        svgLink?.getAttribute('title'),
        control?.textContent,
        control?.getAttribute('aria-label'),
        control?.getAttribute('title'),
        control?.getAttribute('name'),
        control?.getAttribute('value'),
        control?.id,
        control?.className,
        control?.getAttribute('data-testid'),
      ]
      const downloadHint = isLikelyDownloadControl(controlHints)
      const explicitDownloadTarget = Boolean(htmlLink?.hasAttribute('download') || downloadHref)
      if (!shouldTrackDownloadIntent({
        directHref,
        hintedHref: downloadHref || hintedHref,
        ctrlForce: event.ctrlKey,
        explicitDownloadTarget,
        hints: controlHints,
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
      const targetValue = htmlLink && typeof htmlLink.target === 'string'
        ? htmlLink.target
        : svgLink?.target?.baseVal || ''
      void browser.runtime.sendMessage({
        type: 'click-intent',
        href,
        pageUrl: location.href,
        altBypass: event.altKey,
        ctrlForce: event.ctrlKey,
        generic: !href,
        opensNewTab: linkOpensNewTab(targetValue),
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
