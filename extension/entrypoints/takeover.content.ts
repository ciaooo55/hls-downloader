import { browser } from 'wxt/browser'
import { localDownloadIntentUrl } from '../lib/resources'
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
  matchAboutBlank: true,
  matchOriginAsFallback: true,
  runAt: 'document_start',
  main() {
    let lastIntent = { key: '', at: 0 }
    let lastTrustedClick = { at: 0, alt: false, ctrl: false }
    const recordIntent = (event: MouseEvent) => {
      if (event.button !== 0) return
      if (event.isTrusted) lastTrustedClick = { at: Date.now(), alt: event.altKey, ctrl: event.ctrlKey }
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
      const localHref = htmlLink?.href || ''
      if (/^(blob:|data:)/i.test(localHref) && htmlLink?.hasAttribute('download')) {
        // 导出按钮常在可信点击中调用隐藏 a.click()；绑定真实本地 URL，避免缺失 referrer 时丢掉来源和 Alt。
        const modifiers = Date.now() - lastTrustedClick.at < 1_500 ? lastTrustedClick : { alt: false, ctrl: false }
        const key = `${localHref}:${modifiers.alt}:${modifiers.ctrl}`
        if (lastIntent.key === key && Date.now() - lastIntent.at < 1_500) return
        lastIntent = { key, at: Date.now() }
        void localDownloadIntentUrl(localHref).then(href => browser.runtime.sendMessage({
          type: 'click-intent', href, pageUrl: location.href,
          altBypass: modifiers.alt, ctrlForce: modifiers.ctrl, generic: false, controlHint: true,
        }))
        return
      }
      if (!event.isTrusted) return
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
        downloadAttribute: Boolean(htmlLink?.hasAttribute('download')),
      })
    }
    // Many download controls start navigation from pointerdown or tear down the
    // document before their click handler returns. Record strong intent at the
    // earliest trusted phase so onHeadersReceived can present immediately.
    window.addEventListener('pointerdown', recordIntent, true)
    window.addEventListener('click', recordIntent, true)
  },
})
