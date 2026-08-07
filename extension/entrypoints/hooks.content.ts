import { detectManifestKind, manifestMimeType, shouldInspectManifestResponse, shouldReportMediaResponse } from '../lib/manifestSniff'

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
    const mseReportTimes = new Map<string, number>()
    const MSE_REPORT_INTERVAL_MS = 300
    const report = (url: unknown, mimeType = '') => {
      if (typeof url !== 'string') return
      if (!shouldReportMediaResponse(url, mimeType)) return
      pendingResources.push({ url, mimeType })
      if (pendingResources.length > 200) pendingResources.shift()
      window.dispatchEvent(new CustomEvent('__hls_downloader_resource__', { detail: { url, mimeType } }))
    }
    const reportMse = (blobUrl: string, mediaUrl: string) => {
      if (!blobUrl.startsWith('blob:') || !/^https?:/i.test(mediaUrl)) return
      // LL-HLS players append several audio/video chunks per second. Keep the
      // exact ownership signal, but do not make each append redraw the page.
      const now = Date.now()
      const last = mseReportTimes.get(blobUrl) || 0
      if (now - last < MSE_REPORT_INTERVAL_MS) return
      mseReportTimes.set(blobUrl, now)
      if (mseReportTimes.size > 64) {
        for (const [key, reportedAt] of mseReportTimes) {
          if (now - reportedAt > 60_000) mseReportTimes.delete(key)
        }
      }
      const existing = pendingMse.findIndex(item => item.blobUrl === blobUrl && item.mediaUrl === mediaUrl)
      if (existing >= 0) pendingMse.splice(existing, 1)
      pendingMse.push({ blobUrl, mediaUrl })
      if (pendingMse.length > 48) pendingMse.shift()
      window.dispatchEvent(new CustomEvent('__hls_downloader_mse__', {
        detail: { blobUrl, mediaUrl },
      }))
    }
    const inspectManifestResponse = async (response: Response, mimeType: string) => {
      if (!shouldInspectManifestResponse(response.url, mimeType)) return
      const body = response.body
      if (!body) return
      const reader = body.getReader()
      const chunks: Uint8Array[] = []
      let total = 0
      try {
        while (total < 128 * 1024) {
          const next = await reader.read()
          if (next.done || !next.value) break
          const value = next.value instanceof Uint8Array ? next.value : new Uint8Array(next.value)
          const remaining = 128 * 1024 - total
          const chunk = value.byteLength > remaining ? value.slice(0, remaining) : value
          chunks.push(chunk)
          total += chunk.byteLength
          if (chunk.byteLength < value.byteLength) break
        }
      } catch {
        return
      } finally {
        try { await reader.cancel() } catch {}
      }
      if (!total) return
      const prefix = new TextDecoder().decode(
        (() => {
          const bytes = new Uint8Array(total)
          let offset = 0
          chunks.forEach(chunk => { bytes.set(chunk, offset); offset += chunk.byteLength })
          return bytes
        })(),
      )
      const kind = detectManifestKind(prefix)
      if (kind) report(response.url, manifestMimeType(kind))
    }
    const rememberBufferSource = (value: unknown, sourceUrl: string) => {
      if (!sourceUrl || (!value || (typeof value !== 'object' && typeof value !== 'function'))) return
      bufferSources.set(value as object, sourceUrl)
      if (ArrayBuffer.isView(value)) bufferSources.set(value.buffer, sourceUrl)
    }
    window.addEventListener('__hls_downloader_replay__', () => {
      pendingResources.forEach(event => window.dispatchEvent(new CustomEvent('__hls_downloader_resource__', { detail: event })))
      pendingMse.forEach(event => window.dispatchEvent(new CustomEvent('__hls_downloader_mse__', { detail: event })))
    })
    try {
      const notifyNavigation = () => queueMicrotask(() => {
        window.dispatchEvent(new Event('__hls_downloader_navigation__'))
      })
      for (const method of ['pushState', 'replaceState'] as const) {
        const original = history[method]
        history[method] = function (this: History, ...args: Parameters<History['pushState']>) {
          const result = original.apply(this, args)
          notifyNavigation()
          return result
        } as History[typeof method]
      }
    } catch {
      // Frozen History methods only disable immediate SPA navigation signals;
      // popstate/hashchange and media events remain available.
    }
    try {
      const attachShadow = Element.prototype.attachShadow
      Element.prototype.attachShadow = function (init: ShadowRootInit) {
        const root = attachShadow.call(this, init)
        // A host can attach an open root after the isolated content script has
        // completed its initial DOM scan. Signal on the host itself so the
        // other world can recover it through composedPath without passing DOM
        // objects in CustomEvent.detail.
        this.dispatchEvent(new CustomEvent('__hls_downloader_shadow__', {
          bubbles: true,
          composed: true,
        }))
        return root
      }
    } catch {
      // Frozen prototypes only disable late ShadowRoot discovery.
    }
    // MSE libraries do not all consume fetch responses through arrayBuffer().
    // Preserve request ownership through clone/blob/bytes and streaming readers
    // so appendBuffer can still be tied to the correct player.
    const streamSources = new WeakMap<object, string>()
    const readerSources = new WeakMap<object, string>()
    try {
      const responseArrayBuffer = Response.prototype.arrayBuffer
      Response.prototype.arrayBuffer = async function () {
        const value = await responseArrayBuffer.call(this)
        rememberBufferSource(value, this.url)
        return value
      }
      const responseBlob = Response.prototype.blob
      Response.prototype.blob = async function () {
        const value = await responseBlob.call(this)
        rememberBufferSource(value, this.url)
        return value
      }
      const responsePrototype = Response.prototype as Response & { bytes?: () => Promise<Uint8Array> }
      const responseBytes = responsePrototype.bytes
      if (typeof responseBytes === 'function') {
        responsePrototype.bytes = async function (this: Response) {
          const value = await responseBytes.call(this)
          rememberBufferSource(value, this.url)
          return value
        }
      }
      const responseClone = Response.prototype.clone
      Response.prototype.clone = function () {
        const value = responseClone.call(this)
        if (value.body) streamSources.set(value.body, value.url || this.url)
        return value
      }
      const blobArrayBuffer = Blob.prototype.arrayBuffer
      Blob.prototype.arrayBuffer = async function () {
        const value = await blobArrayBuffer.call(this)
        const source = bufferSources.get(this)
        if (source) rememberBufferSource(value, source)
        return value
      }
      // Player libraries commonly normalize or trim fetched bytes before
      // appendBuffer. Preserve ownership across those copies; otherwise one
      // harmless `arrayBuffer.slice()`/`Uint8Array.slice()` turns a precisely
      // identified player back into a page-level guess.
      const arrayBufferSlice = ArrayBuffer.prototype.slice
      ArrayBuffer.prototype.slice = function (start?: number, end?: number) {
        const value = arrayBufferSlice.call(this, start, end)
        const source = bufferSources.get(this)
        if (source) rememberBufferSource(value, source)
        return value
      }
      const blobSlice = Blob.prototype.slice
      Blob.prototype.slice = function (start?: number, end?: number, contentType?: string) {
        const value = blobSlice.call(this, start, end, contentType)
        const source = bufferSources.get(this)
        if (source) rememberBufferSource(value, source)
        return value
      }
      const typedArrayPrototype = Object.getPrototypeOf(Uint8Array.prototype) as {
        slice?: (start?: number, end?: number) => ArrayBufferView
      }
      const typedArraySlice = typedArrayPrototype.slice
      if (typeof typedArraySlice === 'function') {
        typedArrayPrototype.slice = function (this: ArrayBufferView, start?: number, end?: number) {
          const value = typedArraySlice.call(this, start, end)
          const source = bufferSources.get(this) || bufferSources.get(this.buffer)
          if (source) rememberBufferSource(value, source)
          return value
        }
      }
      const getReader = ReadableStream.prototype.getReader
      ReadableStream.prototype.getReader = function (this: ReadableStream<any>, ...args: any[]) {
        const reader = (getReader as any).apply(this, args)
        const source = streamSources.get(this)
        if (source) readerSources.set(reader, source)
        return reader
      } as typeof ReadableStream.prototype.getReader
      const patchReader = (prototype: any) => {
        if (!prototype?.read) return
        const read = prototype.read
        prototype.read = async function (...args: any[]) {
          const result = await read.apply(this, args)
          const source = readerSources.get(this)
          if (source && result && 'value' in result) rememberBufferSource(result.value, source)
          return result
        }
      }
      patchReader(globalThis.ReadableStreamDefaultReader?.prototype)
      patchReader(globalThis.ReadableStreamBYOBReader?.prototype)
    } catch {
      // Frozen prototypes disable only this optional ownership layer.
    }

    const originalFetch = window.fetch
    window.fetch = async function (...args) {
      const response = await originalFetch.apply(this, args)
      const mimeType = response.headers.get('content-type') || ''
      report(response.url, mimeType)
      // Some live CDNs expose an extensionless URL and label the manifest as
      // octet-stream.  A bounded clone lets the isolated world classify it
      // immediately without buffering the real response or touching page
      // playback.  Ordinary MP4s are not cloned unless their URL has a
      // manifest-like hint.
      try { void inspectManifestResponse(response.clone(), mimeType).catch(() => undefined) } catch {
        // A locked/opaque response must never turn the page's successful fetch
        // into a rejected one; the ordinary URL/MIME report remains valid.
      }
      if (response.body && response.url) streamSources.set(response.body, response.url)
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
        const mimeType = this.getResponseHeader('content-type') || ''
        report(responseUrl, mimeType)
        if (shouldInspectManifestResponse(responseUrl, mimeType)
          && (!this.responseType || this.responseType === 'text')) {
          try {
            const kind = detectManifestKind(String(this.responseText || '').slice(0, 128 * 1024))
            if (kind) report(responseUrl, manifestMimeType(kind))
          } catch {
            // Binary XHR responses expose no responseText; fetch/MSE hooks
            // still provide the normal ownership evidence in that case.
          }
        }
        const response = this.response
        if (response && typeof response === 'object') {
          rememberBufferSource(response, responseUrl)
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
            else {
              const source = bufferSources.get(object)
              if (source) reportMse(value, source)
            }
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
