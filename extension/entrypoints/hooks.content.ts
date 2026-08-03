export default defineContentScript({
  matches: ['<all_urls>'],
  allFrames: true,
  world: 'MAIN',
  runAt: 'document_start',
  main() {
    const bufferSources = new WeakMap<object, string>()
    const mediaSourceBlobs = new WeakMap<object, string>()
    const sourceBufferOwners = new WeakMap<object, object>()
    const pendingResources: Array<{ url: string; mimeType: string }> = []
    const pendingMse: Array<{ blobUrl: string; mediaUrl: string }> = []
    const report = (url: unknown, mimeType = '') => {
      if (typeof url !== 'string') return
      pendingResources.push({ url, mimeType })
      if (pendingResources.length > 200) pendingResources.shift()
      window.dispatchEvent(new CustomEvent('__hls_downloader_resource__', { detail: { url, mimeType } }))
    }
    const reportMse = (blobUrl: string, mediaUrl: string) => {
      if (!blobUrl.startsWith('blob:') || !/^https?:/i.test(mediaUrl)) return
      pendingMse.push({ blobUrl, mediaUrl })
      if (pendingMse.length > 200) pendingMse.shift()
      window.dispatchEvent(new CustomEvent('__hls_downloader_mse__', {
        detail: { blobUrl, mediaUrl },
      }))
    }
    window.addEventListener('__hls_downloader_replay__', () => {
      pendingResources.forEach(event => window.dispatchEvent(new CustomEvent('__hls_downloader_resource__', { detail: event })))
      pendingMse.forEach(event => window.dispatchEvent(new CustomEvent('__hls_downloader_mse__', { detail: event })))
    })
    const originalFetch = window.fetch
    window.fetch = async function (...args) {
      const response = await originalFetch.apply(this, args)
      report(response.url, response.headers.get('content-type') || '')
      const sourceUrl = response.url
      const originalArrayBuffer = response.arrayBuffer.bind(response)
      try {
        response.arrayBuffer = async () => {
          const value = await originalArrayBuffer()
          if (sourceUrl) bufferSources.set(value, sourceUrl)
          return value
        }
      } catch {
        // Some sites freeze Response instances. Observation must never change
        // whether their fetch succeeds.
      }
      return response
    }
    const open = XMLHttpRequest.prototype.open
    const invokeOpen = open as unknown as (
      this: XMLHttpRequest,
      method: string,
      url: string | URL,
      async?: boolean,
      username?: string | null,
      password?: string | null,
    ) => void
    XMLHttpRequest.prototype.open = function (
      method: string,
      url: string | URL,
      async?: boolean,
      username?: string | null,
      password?: string | null,
    ) {
      this.addEventListener('load', () => {
        const responseUrl = this.responseURL || String(url)
        report(responseUrl, this.getResponseHeader('content-type') || '')
        const response = this.response
        if (response && typeof response === 'object') {
          bufferSources.set(response, responseUrl)
        }
      })
      if (async === undefined) return invokeOpen.call(this, method, url)
      return invokeOpen.call(this, method, url, async, username, password)
    }

    // Associate bytes appended to one SourceBuffer with the MediaSource blob
    // used by a concrete video element. This recovers information hidden by
    // blob: currentSrc and lets the isolated content script distinguish two
    // simultaneous MSE players when their segment paths differ.
    if (typeof MediaSource !== 'undefined' && typeof SourceBuffer !== 'undefined') {
      try {
        const addSourceBuffer = MediaSource.prototype.addSourceBuffer
        MediaSource.prototype.addSourceBuffer = function (mimeType: string) {
          const sourceBuffer = addSourceBuffer.call(this, mimeType)
          sourceBufferOwners.set(sourceBuffer, this)
          return sourceBuffer
        }
        const appendBuffer = SourceBuffer.prototype.appendBuffer
        SourceBuffer.prototype.appendBuffer = function (data: BufferSource) {
          try {
            const source = bufferSources.get(data as object)
              || (ArrayBuffer.isView(data) ? bufferSources.get(data.buffer) : '')
            const owner = sourceBufferOwners.get(this)
            const blobUrl = owner ? mediaSourceBlobs.get(owner) || '' : ''
            if (source && blobUrl) reportMse(blobUrl, source)
          } catch {
            // Preserve the page's original append behavior on every observer
            // failure; the extension may miss evidence but cannot break video.
          }
          return appendBuffer.call(this, data)
        }
        const createObjectURL = URL.createObjectURL.bind(URL)
        URL.createObjectURL = function (object: Blob | MediaSource): string {
          const value = createObjectURL(object)
          try {
            if (object instanceof MediaSource) mediaSourceBlobs.set(object, value)
          } catch {}
          return value
        }
      } catch {
        // Frozen browser/page prototypes disable this optional correlation
        // layer; fetch/XHR/media detection above remains fully functional.
      }
    }
  },
})
