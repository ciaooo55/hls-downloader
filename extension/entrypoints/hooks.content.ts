export default defineContentScript({
  matches: ['<all_urls>'],
  world: 'MAIN',
  runAt: 'document_start',
  main() {
    const report = (url: unknown, mimeType = '') => {
      if (typeof url !== 'string') return
      window.dispatchEvent(new CustomEvent('__hls_downloader_resource__', { detail: { url, mimeType } }))
    }
    const originalFetch = window.fetch
    window.fetch = async function (...args) {
      const response = await originalFetch.apply(this, args)
      report(response.url, response.headers.get('content-type') || '')
      return response
    }
    const open = XMLHttpRequest.prototype.open
    XMLHttpRequest.prototype.open = function (method, url, ...args) {
      this.addEventListener('load', () => report(this.responseURL || String(url), this.getResponseHeader('content-type') || ''))
      return open.call(this, method, url, ...args)
    }
  },
})
