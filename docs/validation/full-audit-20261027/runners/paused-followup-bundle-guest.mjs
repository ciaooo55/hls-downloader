// Run built background scripts in Sandbox with deterministic browser/Host fixtures.
import assert from 'node:assert/strict'
import { createHash, webcrypto } from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'
import vm from 'node:vm'

assert.equal(process.env.USERNAME, 'WDAGUtilityAccount', 'Windows Sandbox only')
const input = 'C:\\QAFollowupInput'
const reports = 'C:\\QAFollowupReports'
const work = 'C:\\QAFollowupWork'
const raceOnly = process.argv.includes('--ownership-race')
const localImport = process.argv.includes('--local-import')
fs.mkdirSync(work, { recursive: true })
const result = { passed: false, finished: false, scope: 'Built background scripts; browser APIs and Native Host simulated; no real browser or UI acceptance', checks: [] }
const hash = value => createHash('sha256').update(value).digest('hex')
const event = () => ({ listeners: [], addListener(fn) { this.listeners.push(fn) } })
const drain = async () => { for (let index = 0; index < 160; index++) await Promise.resolve() }

async function check(family, scenario) {
  let now = 1_000_000
  let timerId = 0
  const timers = new Map()
  const calls = []
  const ownershipRace = scenario.startsWith('user-')
  let recovering = false
  let storageReadFailed = false
  let pendingReceipt
  const canary = path.join(work, `${family}-${scenario}.bin`)
  const payload = Buffer.from('Browser file must survive a failed desktop confirmation.\n'.repeat(128))
  fs.writeFileSync(canary, payload)
  const imported = path.join(work, `${family}-${scenario}-imported.bin`)
  const item = { id: 42, url: 'https://fixture.invalid/export', state: localImport || scenario === 'complete' ? 'complete' : 'in_progress', paused: !localImport && scenario !== 'complete', filename: canary, bytesReceived: payload.length, totalBytes: payload.length, fileSize: payload.length }
  let erased = false
  const key = localImport ? 'local-browser-download-handoffs' : 'paused-handoff-followups-v1'
  const local = { desktopClientId: 'followup-fixture', [key]: localImport ? [{
    downloadId: 42, requestId: 'original-local-request', sourceKey: `https://local-download.invalid/${hash(item.url)}`,
    pageUrl: 'https://fixture.invalid/', importUrl: 'data:application/octet-stream,',
  }] : [{
    downloadId: 42, handoffId: '', clientRequestId: 'original-request', phase: 'resolution',
    createdAt: now - 60_000, deadline: scenario === 'cadence' ? now + 60_000 : now - 1,
  }] }
  const storage = values => ({
    async get(key) {
      if (values === local && scenario === 'local-storage' && key === 'local-browser-download-handoffs' && !storageReadFailed) {
        storageReadFailed = true
        throw new Error('transient storage read failure')
      }
      return key == null ? { ...values } : { [key]: values[key] }
    },
    async set(items) { Object.assign(values, items) },
    async remove(keys) { for (const key of Array.isArray(keys) ? keys : [keys]) delete values[key] },
  })
  const manifest = JSON.parse(fs.readFileSync(path.join(input, `${family}-manifest.json`)))
  const browser = {
    runtime: {
      id: 'followup-fixture', getManifest: () => manifest,
      getURL: value => `chrome-extension://followup-fixture/${value}`,
      onInstalled: event(), onMessage: event(),
      connectNative() {
        const port = { onMessage: event(), onDisconnect: event(), disconnect() {}, postMessage(message) {
          calls.push({ op: message.op, at: now, requestId: message.client_request_id || message.resource?.client_request_id })
          if (ownershipRace && message.op === 'handoff_status') {
            pendingReceipt = { port, message }
            return
          }
          let reply = message.op === 'ping' ? { ok: true, takeover_enabled: true, takeover_minimum_bytes: 0 }
            : message.op === 'handoff_status' ? { ok: true, handoff: scenario === 'cadence' ? null : { id: 'failed-confirmation', status: 'failed', presentation_ok: false } }
              : { ok: false, error: 'unexpected fixture operation' }
          if (localImport && message.op === 'offer_local_file') {
            reply = !recovering && ['local-transport', 'local-empty'].includes(scenario)
              ? scenario === 'local-empty' ? { ok: true } : { ok: false, error: 'Core pipe read: closed' }
              : { ok: true, handoff: { id: 'local-receipt', status: 'pending' } }
          }
          if (localImport && message.op === 'handoff_status') {
            if (recovering) {
              // Host is simulated. The real sandbox files verify that browser
              // cleanup happens only after the fixture reports a finished copy.
              if (!fs.existsSync(imported)) fs.copyFileSync(canary, imported)
              reply = { ok: true, handoff: { id: 'local-receipt', status: 'accepted', task_status: 'completed' } }
            } else reply = { ok: false, error: 'Core pipe read: closed' }
          }
          reply = { ...reply, __request_id: message.__request_id }
          queueMicrotask(() => port.onMessage.listeners.forEach(fn => fn(reply)))
        } }
        return port
      },
    },
    tabs: { onUpdated: event(), onActivated: event(), onRemoved: event(), async get() { return { url: 'https://fixture.invalid/' } }, async query() { return [] }, async sendMessage() {} },
    contextMenus: { onClicked: event(), async removeAll() {}, async create() {} },
    alarms: { onAlarm: event(), async create() {}, async clear() { return true } },
    action: { async setBadgeText() {} },
    cookies: { async getAll() { return [] } },
    webRequest: Object.fromEntries(['onSendHeaders', 'onBeforeRequest', 'onBeforeRedirect', 'onHeadersReceived', 'onCompleted', 'onErrorOccurred'].map(key => [key, event()])),
    storage: { local: storage(local), session: storage({}) },
    downloads: {
      onCreated: event(), onChanged: event(), async search() { return erased ? [] : [{ ...item }] },
      async resume() { calls.push({ op: 'resume' }); item.paused = false },
      async cancel() { calls.push({ op: 'cancel' }); item.state = 'interrupted' },
      async removeFile() { calls.push({ op: 'remove-file' }); fs.unlinkSync(canary) },
      async erase() { calls.push({ op: 'erase' }); erased = true; return [42] },
    },
  }
  const source = fs.readFileSync(path.join(input, `${family}-background.js`), 'utf8')
  const context = vm.createContext({
    browser, chrome: browser,
    // Keep SHA-256 identical while removing OS thread-pool timing from the
    // virtual clock, as in the corresponding ownership unit-test fixture.
    crypto: { randomUUID: () => webcrypto.randomUUID(), subtle: { async digest(algorithm, data) {
      assert.equal(algorithm, 'SHA-256')
      return Uint8Array.from(createHash('sha256').update(Buffer.from(data)).digest()).buffer
    } } },
    URL, URLSearchParams, TextEncoder, TextDecoder, console,
    navigator: { userAgent: family === 'firefox' ? 'Firefox/158.0' : 'Edg/156.0' },
    Date: class extends Date { static now() { return now } },
    setTimeout(fn, delay = 0) { const id = ++timerId; timers.set(id, { at: now + Math.max(0, delay), fn }); return id },
    clearTimeout(id) { timers.delete(id) },
    queueMicrotask,
  })
  const advance = async duration => {
    const target = now + duration
    let steps = 0
    for (;;) {
      await drain()
      const next = [...timers].filter(([, timer]) => timer.at <= target).sort((a, b) => a[1].at - b[1].at)[0]
      if (!next) break
      assert.ok(++steps < 1000, 'unexpected timer loop')
      timers.delete(next[0]); now = next[1].at; next[1].fn()
    }
    now = target
    await drain()
  }
  vm.runInContext(source, context, { timeout: 5000 })
  await drain()
  if (localImport) {
    assert.ok(fs.existsSync(canary), 'source was deleted before a successful import receipt')
    const offers = () => calls.filter(call => call.op === 'offer_local_file')
    const queries = () => calls.filter(call => call.op === 'handoff_status')
    if (scenario === 'local-storage') {
      assert.ok(storageReadFailed)
      assert.equal(offers().length, 0)
      recovering = true
      for (const listener of browser.alarms.onAlarm.listeners) listener({ name: 'desktop-heartbeat' })
      await drain()
    } else {
      await advance(500)
      const initialOffers = offers().length
      const initialQueries = queries().length
      assert.ok(initialOffers > 0, 'native import offer was never reached')
      assert.equal(local[key].length, 1, 'unknown import lost its durable recovery record')
      await advance(30_000)
      assert.equal(offers().length, initialOffers, 'import retried before its deadline')
      assert.equal(queries().length, initialQueries, 'failed status kept polling')
      assert.ok(fs.existsSync(canary))
      recovering = true
      await advance(30_000)
    }
    assert.equal(local[key].length, 0)
    assert.equal(new Set(offers().map(call => call.requestId)).size, 1)
    assert.equal(offers()[0].requestId, 'original-local-request')
    assert.equal(calls.filter(call => call.op === 'remove-file').length, 1)
    assert.equal(calls.filter(call => call.op === 'erase').length, 1)
    assert.equal(calls.filter(call => ['resume', 'cancel', 'offer', 'recover_offer'].includes(call.op)).length, 0)
    assert.ok(!fs.existsSync(canary))
    assert.equal(hash(fs.readFileSync(imported)), hash(payload))
    return { family, scenario, passed: true, background_sha256: hash(source), imported_file_sha256: hash(payload), calls }
  } else if (ownershipRace) {
    assert.ok(pendingReceipt, 'receipt lookup did not start')
    item.paused = false
    item.state = scenario === 'user-resume' ? 'in_progress' : 'interrupted'
    const { port, message } = pendingReceipt
    port.onMessage.listeners.forEach(fn => fn({ ok: true, __request_id: message.__request_id, handoff: { id: 'recovered', status: 'pending' } }))
    await drain()
    assert.equal(local[key].length, 0, 'stale browser ownership remained in follow-up queue')
    assert.equal(calls.filter(call => call.op === 'resume').length, 0, 'overrode browser-side choice')
  } else if (scenario === 'cadence') {
    const queries = () => calls.filter(call => call.op === 'handoff_status').length
    assert.equal(queries(), 0)
    await advance(30_000)
    for (const listener of browser.alarms.onAlarm.listeners) listener({ name: 'desktop-heartbeat' })
    await drain()
    assert.equal(queries(), 0, 'heartbeat queried a future deadline')
    await advance(30_000)
    assert.equal(queries(), 1)
    await advance(60_000)
    assert.equal(queries(), 2, 'resolution retry accelerator was not rearmed')
  } else {
    assert.equal(local[key].length, 0)
    assert.equal(item.paused, false)
    assert.equal(calls.filter(call => call.op === 'resume').length, scenario === 'complete' ? 0 : 1)
  }
  assert.equal(calls.filter(call => ['remove-file', 'cancel', 'erase', 'offer', 'recover_offer'].includes(call.op)).length, 0)
  assert.equal(hash(fs.readFileSync(canary)), hash(payload), 'browser file was removed or modified')
  return { family, scenario, passed: true, background_sha256: hash(source), file_sha256: hash(payload), calls }
}

try {
  const scenarios = localImport ? ['local-transport', 'local-empty', 'local-storage', 'local-status']
    : raceOnly ? ['user-resume', 'user-cancel'] : ['in-progress', 'complete', 'cadence']
  for (const family of ['chrome', 'firefox']) {
    for (const scenario of scenarios) result.checks.push(await check(family, scenario))
  }
  result.passed = true
} catch (error) {
  result.error = String(error.stack || error)
  process.exitCode = 1
} finally {
  result.finished = true
  fs.writeFileSync(path.join(reports, localImport ? 'local-import-summary.json' : raceOnly ? 'ownership-race-summary.json' : 'summary.json'), JSON.stringify(result, null, 2) + '\n')
}
