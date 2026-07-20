import { browser } from 'wxt/browser'
import { classifyDownload, isDirectDownloadLink, resourceId, type MediaResource } from '../lib/resources'

export default defineContentScript({
  matches: ['<all_urls>'],
  runAt: 'document_start',
  main() {
    window.addEventListener('click', event => {
      if (!event.isTrusted || event.button !== 0) return
      const anchor = event.composedPath()
        .find(value => value instanceof HTMLAnchorElement) as HTMLAnchorElement | undefined
      if (!anchor?.href) return

      const hasDownloadAttribute = anchor.hasAttribute('download')
      const preempt = !event.altKey && (event.ctrlKey || isDirectDownloadLink(anchor.href, hasDownloadAttribute))
      const intent = {
        type: 'click-intent',
        href: anchor.href,
        pageUrl: location.href,
        altBypass: event.altKey,
        ctrlForce: event.ctrlKey,
      }
      if (!preempt) {
        void browser.runtime.sendMessage(intent)
        return
      }

      event.preventDefault()
      event.stopImmediatePropagation()
      const filename = anchor.download || ''
      const resource: MediaResource = {
        id: resourceId(anchor.href),
        url: anchor.href,
        kind: classifyDownload(anchor.href, '', filename) || 'file',
        filename,
        title: filename || anchor.textContent?.trim() || '',
        pageUrl: location.href,
        seenAt: Date.now(),
      }
      void browser.runtime.sendMessage({ type: 'preempt-download', resource, intent })
    }, true)
  },
})
