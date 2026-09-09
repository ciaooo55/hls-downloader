/** Keep popup/content actions responsive when MV3 or Native Messaging stalls. */
export function withDeadline<T>(
  operation: Promise<T>,
  timeoutMs: number,
  timeoutMessage = '操作超时，请重试',
): Promise<T> {
  const delay = Math.max(1, Math.floor(timeoutMs))
  return new Promise<T>((resolve, reject) => {
    const timer = globalThis.setTimeout(() => reject(new Error(timeoutMessage)), delay)
    operation.then(
      value => { globalThis.clearTimeout(timer); resolve(value) },
      reason => { globalThis.clearTimeout(timer); reject(reason) },
    )
  })
}
