/** Gate so the main window never emits a handoff into a WebView that is not listening. */
export function createHandoffHostReady(timeoutMs = 15_000) {
  let ready = false
  let resolveReady: (() => void) | null = null
  const readyPromise = new Promise<void>(resolve => {
    resolveReady = resolve
  })

  const markReady = () => {
    ready = true
    resolveReady?.()
  }

  const wait = async () => {
    if (ready) return
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      await Promise.race([
        readyPromise,
        new Promise<void>((_, reject) => {
          timer = setTimeout(() => reject(new Error('下载确认窗口未就绪')), timeoutMs)
        }),
      ])
    } finally {
      if (timer !== undefined) clearTimeout(timer)
    }
  }

  return {
    markReady,
    wait,
    get ready() {
      return ready
    },
  }
}
