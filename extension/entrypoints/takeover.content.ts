import { browser } from 'wxt/browser'
import { localDownloadIntentUrl } from '../lib/resources'
import { isDirectDownloadLink } from '../lib/directLink'
import { desktopAcceptedHandoff } from '../lib/takeover'
import {
  isForceTakeoverKey,
  isLikelyDownloadControl,
  linkOpensNewTab,
  resolveClickedLinkHref,
  resolveDownloadTarget,
  resolveFormDownloadUrl,
  resolveTakeoverClickModifiers,
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
  main(ctx) {
    let interceptLinks = false
    let linkSettings = { enabled: true, minimumBytes: 0 }
    if (import.meta.env.CHROME) {
      const applySettings = () => { interceptLinks = linkSettings.enabled && linkSettings.minimumBytes <= 0 }
      void browser.storage.local.get(['enabled', 'minimumBytes']).then(values => {
        linkSettings = { enabled: values.enabled !== false, minimumBytes: Number(values.minimumBytes || 0) }
        applySettings()
      }).catch(() => undefined)
      const onSettingsChanged = (changes: Record<string, { newValue?: unknown }>, area: string) => {
        if (area !== 'local') return
        if (changes.enabled) linkSettings.enabled = changes.enabled.newValue !== false
        if (changes.minimumBytes) linkSettings.minimumBytes = Number(changes.minimumBytes.newValue || 0)
        applySettings()
      }
      browser.storage.onChanged.addListener(onSettingsChanged)
      ctx.onInvalidated(() => browser.storage.onChanged.removeListener(onSettingsChanged))
    }
    let insertHeld = false
    ctx.addEventListener(window, 'keydown', (event: KeyboardEvent) => {
      if (isForceTakeoverKey(event)) insertHeld = true
    }, { capture: true })
    ctx.addEventListener(window, 'keyup', (event: KeyboardEvent) => {
      if (isForceTakeoverKey(event)) insertHeld = false
    }, { capture: true })
    ctx.addEventListener(window, 'blur', () => {
      insertHeld = false
    })
    ctx.addEventListener(document, 'visibilitychange', () => {
      if (document.visibilityState !== 'visible') insertHeld = false
    })
    let lastIntent = { key: '', at: 0 }
    let lastTrustedClick = { at: 0, alt: false, ctrl: false }
    const recordIntent = (event: MouseEvent) => {
      if (event.button !== 0) return
      const { altBypass, ctrlForce } = resolveTakeoverClickModifiers({
        altKey: event.altKey,
        insertHeld,
      })
      if (event.isTrusted) lastTrustedClick = { at: Date.now(), alt: altBypass, ctrl: ctrlForce }
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
        if (!event.isTrusted && Date.now() - lastTrustedClick.at >= 1_500) return
        const modifiers = lastTrustedClick
        const key = `${localHref}:${modifiers.alt}:${modifiers.ctrl}`
        if (lastIntent.key === key && Date.now() - lastIntent.at < 1_500) return
        lastIntent = { key, at: Date.now() }
        void localDownloadIntentUrl(localHref).then(href => browser.runtime.sendMessage({
          type: 'click-intent', href, pageUrl: location.href,
          altBypass: modifiers.alt, ctrlForce: modifiers.ctrl, generic: false, controlHint: true,
        })).catch(() => undefined)
        return
      }
      if (!event.isTrusted) return
      const directHref = resolveClickedLinkHref({
        htmlHref: htmlLink?.href || '',
        htmlHrefAttribute: htmlLink ? attribute(htmlLink, 'href') : '',
        svgHrefAttribute: svgLink ? attribute(svgLink, 'href') : '',
        svgXlinkHref: svgLink ? attribute(svgLink, 'xlink:href') : '',
        svgBaseVal: svgLink?.href?.baseVal || '',
        baseUrl: document.baseURI,
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
        document.baseURI,
      )
      const downloadHref = resolveDownloadTarget(rawDownloadHref, document.baseURI)
      const hintedHref = resolveDownloadTarget(rawHintedHref, document.baseURI) || formActionHref
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
        altBypass,
        ctrlForce,
        explicitDownloadTarget,
        hints: controlHints,
      })) return
      const href = downloadHref || directHref || hintedHref
      const intentKey = [href, location.href, altBypass ? 1 : 0, ctrlForce ? 1 : 0, downloadHint ? 1 : 0].join('|')
      const now = Date.now()
      // A pointerdown followed by click describes one user action. Deduplicate
      // only the paired click so a fresh pointerdown (e.g. a second Alt+Click
      // on the same link) always records its own intent. Keyboard activation
      // has no pointerdown and still reaches the click listener normally.
      if (event.type === 'click' && lastIntent.key === intentKey && now - lastIntent.at < 1_500) return
      lastIntent = { key: intentKey, at: now }
      const targetValue = htmlLink && typeof htmlLink.target === 'string'
        ? htmlLink.target
        : svgLink?.target?.baseVal || ''
      void browser.runtime.sendMessage({
        type: 'click-intent',
        href,
        pageUrl: location.href,
        altBypass,
        ctrlForce,
        generic: !href,
        opensNewTab: linkOpensNewTab(targetValue),
        controlHint: downloadHint,
        downloadAttribute: Boolean(htmlLink?.hasAttribute('download')),
        filename: htmlLink?.getAttribute('download') || '',
      }).catch(() => undefined)
    }
    // Many download controls start navigation from pointerdown or tear down the
    // document before their click handler returns. Record strong intent at the
    // earliest trusted phase so onHeadersReceived can present immediately.
    ctx.addEventListener(window, 'pointerdown', recordIntent, { capture: true })
    ctx.addEventListener(window, 'click', recordIntent, { capture: true })
    // 在冒泡末端接管：网页可能先取消默认导航，生成附件地址或更新签名链接。
    ctx.addEventListener(window, 'click', (event: MouseEvent) => {
      const { altBypass, ctrlForce } = resolveTakeoverClickModifiers({
        altKey: event.altKey,
        insertHeld,
      })
      const allowPreempt = import.meta.env.CHROME && (interceptLinks || ctrlForce)
      if (!allowPreempt || !event.isTrusted || !event.cancelable || event.defaultPrevented
        || event.button !== 0 || altBypass || event.shiftKey || event.metaKey || event.ctrlKey) return
      const link = event.composedPath().find(isHtmlLink)
      if (!link || !isDirectDownloadLink(link.href, location.href, link.hasAttribute('download'))) return
      const href = link.href
      const pageUrl = location.href
      const target = link.target || '_self'
      const filename = link.getAttribute('download')
      event.preventDefault()
      const fallback = async (preserveLocal = false) => {
        // 本次交接未发生；若需浏览器带 Cookie 完成再导入，不设 altBypass，让 downloads.onCreated 走本地导入。
        await browser.runtime.sendMessage({
          type: 'click-intent',
          href,
          pageUrl,
          altBypass: !preserveLocal,
          ctrlForce: Boolean(ctrlForce),
          opensNewTab: linkOpensNewTab(target),
          ...(preserveLocal ? { downloadAttribute: filename !== null, filename: filename || '' } : {}),
        }).catch(() => undefined)
        const original = document.createElement('a')
        original.href = href
        original.target = target
        original.rel = link.rel
        original.referrerPolicy = link.referrerPolicy
        if (filename !== null) original.setAttribute('download', filename)
        original.hidden = true
        document.documentElement.append(original)
        original.click()
        original.remove()
      }
      void browser.runtime.sendMessage({
        type: 'intercept-download-link', href, pageUrl, filename,
        downloadAttribute: filename !== null,
        ctrlForce,
      }).then(response => {
        if (!desktopAcceptedHandoff(response) && response?.browserFallback === true) {
          return fallback(response?.preserveLocal === true)
        }
      }).catch(() => {
        // MV3 Service Worker 唤醒失败或扩展重载时回退到浏览器原生点击，避免吞掉用户点击。
        void fallback()
      })
    })
  },
})
