import { beforeEach, describe, expect, it, vi } from 'vitest'

const runtime = vi.hoisted(() => ({ origin: 'http://127.0.0.1:8765' }))

vi.mock('./tauri', () => ({
  coreOrigin: () => runtime.origin,
  internalCredential: () => 'test-credential',
  prepareTauriRuntime: vi.fn(),
}))

import { playbackMediaUrl, taskFileUrl } from './api'

describe('API URLs follow the resolved core port', () => {
  beforeEach(() => {
    runtime.origin = 'http://127.0.0.1:8765'
  })

  it('does not keep the module-load default after the runtime port changes', () => {
    runtime.origin = 'http://127.0.0.1:29991'
    expect(taskFileUrl('task-1', 'file-token')).toContain(
      'http://127.0.0.1:29991/api/tasks/task-1/file',
    )
    expect(playbackMediaUrl('task-1', 'session', 'playback-token')).toContain(
      'http://127.0.0.1:29991/api/tasks/task-1/playback/media',
    )
  })
})
