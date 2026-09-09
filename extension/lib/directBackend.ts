export interface DirectBackendIdentity {
  version: string
  client_id: string
  browser: string
}

/**
 * v7 talks to the resident Rust Core exclusively through Native Messaging.
 *
 * Keep these exports temporarily while background.ts is simplified in a
 * follow-up change, but never attach or route through the retired 5.x/v6
 * FastAPI loopback bridge.
 */
export function shouldClearLoopbackBridge(_response: unknown): boolean {
  return true
}

export function shouldAttachLoopbackBridge(_response: unknown): boolean {
  return false
}

export function shouldRouteThroughLoopbackBridge(_op: unknown, _hasLoopback: boolean): boolean {
  return false
}

export class BrowserDirectBackend {
  constructor(_base: string, _token: string) {}

  async request(
    _message: Record<string, any>,
    _identity: DirectBackendIdentity,
    _timeoutMs = 4_000,
  ): Promise<never> {
    throw new Error('Legacy loopback backend is disabled in v7')
  }
}
