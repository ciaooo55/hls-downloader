import { describe, expect, it } from 'vitest'
import { launchDesktop, launchFailureCode, launchView, type LaunchState } from './desktopLaunch'

function clock() {
  let time = 0
  return { now: () => time, sleep: async (ms: number) => { time += ms } }
}

async function run(responses: Array<any | Error>, options: { timeoutMs?: number; resumeFrom?: LaunchState['pending']; tick?: number } = {}) {
  const states: LaunchState[] = []
  const sent: Array<Record<string, unknown>> = []
  const time = clock()
  const result = await launchDesktop({
    send: async message => {
      sent.push(message)
      time.sleep(options.tick ?? 0)
      const next = responses.shift()
      if (next instanceof Error) throw next
      return next
    },
    onState: state => states.push(state),
    now: time.now,
    sleep: time.sleep,
    timeoutMs: options.timeoutMs,
    resumeFrom: options.resumeFrom,
  })
  return { result, states, sent }
}

describe('launchDesktop', () => {
  it('reports 已打开 only after the workbench readiness receipt', async () => {
    const { result, states, sent } = await run([
      { ok: true, stage: 'launching', launch_id: 'current-launch', since_sequence: 41, verifiable: true },
      { ok: true, ready: false, failed: false, since_sequence: 43 },
      { ok: true, ready: true },
    ])
    expect(states.map(state => state.stage)).toEqual(['connecting', 'launching', 'ready'])
    expect(result.stage).toBe('ready')
    expect(sent[1]).toEqual({ type: 'workbench-status', launchId: 'current-launch', sinceSequence: 41 })
    expect(sent[2]).toEqual({ type: 'workbench-status', launchId: 'current-launch', sinceSequence: 43 })
  })

  it('maps a missing native host to an install action', async () => {
    const missing = Object.assign(new Error('未检测到桌面端'), { code: 'host_missing' })
    const { result } = await run([missing])
    expect(result).toMatchObject({ stage: 'failed', failure: 'host_missing' })
    expect(launchView(result).actions).toEqual(['install', 'retry'])
  })

  it('distinguishes Core, spawn and workbench failures', async () => {
    expect((await run([{ ok: false, code: 'core_unreachable', error: 'x' }])).result.failure).toBe('core_unreachable')
    expect((await run([{ ok: false, code: 'workbench_spawn_failed', error: '找不到程序' }])).result).toMatchObject({
      failure: 'workbench_spawn_failed', detail: '找不到程序',
    })
    const crashed = await run([
      { ok: true, stage: 'launching', launch_id: 'current-launch', since_sequence: 5, verifiable: true },
      { ok: true, ready: false, failed: true },
    ])
    expect(crashed.result.failure).toBe('workbench_failed')
  })

  it('times out with a keep-waiting action that resumes from the last sequence', async () => {
    const waiting = { ok: true, ready: false, failed: false, since_sequence: 9 }
    const { result } = await run([
      { ok: true, stage: 'launching', launch_id: 'current-launch', since_sequence: 7, verifiable: true },
      ...Array.from({ length: 20 }, () => waiting),
    ], { timeoutMs: 3_000, tick: 1_200 })
    expect(result).toMatchObject({ stage: 'failed', failure: 'timeout', pending: { launchId: 'current-launch', sinceSequence: 9 } })
    expect(launchView(result).actions[0]).toBe('keep_waiting')

    const resumed = await run([{ ok: true, ready: true }], { resumeFrom: result.pending })
    expect(resumed.sent).toEqual([{ type: 'workbench-status', launchId: 'current-launch', sinceSequence: 9 }])
    expect(resumed.result.stage).toBe('ready')
  })

  it('never claims 已打开 when an old Core cannot report readiness', async () => {
    const { result } = await run([{ ok: true, activated: true }])
    expect(result.stage).toBe('requested')
    expect(launchView(result).title).not.toContain('已打开')
  })

  it('asks to reopen if the Core no longer has the matching launch history', async () => {
    const { result } = await run([
      { ok: true, stage: 'launching', launch_id: 'current-launch', since_sequence: 5 },
      { ok: false, code: 'launch_status_lost' },
    ])
    expect(result.failure).toBe('launch_status_lost')
    expect(launchView(result).actions).toEqual(['retry'])
  })

  it('treats a lost port mid-wait as a connection failure, not success', async () => {
    const lost = Object.assign(new Error('下载器连接已断开'), { code: 'host_disconnected' })
    const { result } = await run([{ ok: true, stage: 'launching', launch_id: 'current-launch', since_sequence: 3, verifiable: true }, lost])
    expect(result).toMatchObject({ stage: 'failed', failure: 'host_disconnected', pending: { launchId: 'current-launch', sinceSequence: 3 } })
  })
})

describe('launchView', () => {
  it('gives every failure a title, detail and at least one recovery action', () => {
    for (const failure of ['host_missing', 'host_forbidden', 'host_disconnected', 'core_unreachable', 'workbench_spawn_failed', 'workbench_failed', 'timeout', 'unknown'] as const) {
      const view = launchView({ stage: 'failed', failure })
      expect(view.tone).toBe('error')
      expect(view.title.length).toBeGreaterThan(0)
      expect(view.detail.length).toBeGreaterThan(0)
      expect(view.actions.length).toBeGreaterThan(0)
    }
    expect(new Set(['host_missing', 'core_unreachable', 'workbench_spawn_failed', 'timeout']
      .map(failure => launchView({ stage: 'failed', failure: failure as any }).title)).size).toBe(4)
  })

  it('rejects unknown codes', () => {
    expect(launchFailureCode('drop_table')).toBe('unknown')
    expect(launchFailureCode(undefined)).toBe('unknown')
  })
})
