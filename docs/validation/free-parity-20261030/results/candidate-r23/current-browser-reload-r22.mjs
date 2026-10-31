// Actual Edge, extension, Native Host and Core. CDP invokes protocol/DOM only;
// no desktop mouse/keyboard input and no claim of physical-button acceptance.
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import net from 'node:net'
import http from 'node:http'
import { spawn, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { setTimeout as delay } from 'node:timers/promises'
import { startFirefox } from './firefox-marionette.mjs'

assert.equal(process.env.USERNAME, 'WDAGUtilityAccount', 'Windows Sandbox only')
const work = process.env.HLS_CURRENT_WORK_DIR || 'C:\\QACurrentWork'
const firefox = process.argv.includes('--firefox')
const report = path.join(process.env.HLS_CURRENT_REPORT_DIR || 'C:\\QACurrentReports', firefox ? 'firefox.json' : 'edge.json')
const resources = process.env.HLS_CURRENT_RESOURCES || path.join(work, 'product', 'app', 'resources')
const extension = firefox ? 'C:\\QACurrentExtensions\\firefox-mv3' : 'C:\\QACurrentExtensions\\chrome-mv3'
const extensionId = firefox ? 'hls-downloader-store@ciaooo55.com' : 'bbdfldcjnikaemnimalegbopgaknjhla'
const restartOnly = process.argv.includes('--restart-only')
const offlineOnly = process.argv.includes('--offline-only')
const sha256 = data => createHash('sha256').update(data).digest('hex')
const result = { passed: false, finished: false, started_at: new Date().toISOString(), offline_only: offlineOnly, scope: offlineOnly ? 'Actual Edge/current extension; registered Native Host absent; isolated actual Core observes no admissions; no desktop input' : 'Actual Edge + current extension/Core/Host; same candidate installed Presenter; protocol acceptance, no desktop input', checks: [], server_requests: [], browser_events: [] }
const save = phase => { result.phase = phase; fs.writeFileSync(report, JSON.stringify(result, null, 2) + '\n') }
const check = (name, actual) => { result.checks.push({ name, actual }); save(name) }
async function waitFor(fn, name, timeout = 20000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    const value = await fn()
    if (value) return value
    await delay(100)
  }
  throw new Error(`Timed out: ${name}`)
}
function core(message) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(process.env.HLS_V7_PIPE)
    let buffer = Buffer.alloc(0)
    const timer = setTimeout(() => socket.destroy(new Error('Core response timeout')), 5000)
    socket.on('error', reject)
    socket.on('close', () => clearTimeout(timer))
    socket.on('connect', () => {
      const body = Buffer.from(JSON.stringify({ request_id: 1, ...message }))
      const header = Buffer.alloc(4); header.writeUInt32LE(body.length)
      socket.write(Buffer.concat([header, body]))
    })
    socket.on('data', data => {
      buffer = Buffer.concat([buffer, data])
      if (buffer.length < 4) return
      const length = buffer.readUInt32LE(0)
      if (length > 4 * 1024 * 1024) { socket.destroy(new Error('Invalid Core frame')); return }
      if (buffer.length < length + 4) return
      socket.end()
      try {
        const response = JSON.parse(buffer.subarray(4, length + 4))
        if (response.type === 'error') throw new Error(JSON.stringify(response))
        resolve(response)
      } catch (error) { reject(error) }
    })
  })
}

let browser, engine, ws, server
let firefoxSession
let nextId = 0
const pending = new Map()
function cdp(method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const id = ++nextId
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method} (id=${id}, session=${sessionId ?? 'browser'})`)) }, 15000)
    pending.set(id, { resolve, reject, timer })
    ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }))
  })
}
async function evaluate(sessionId, expression) {
  result.last_evaluation = expression.slice(0, 180)
  if (firefox) return firefoxSession.evaluate(sessionId, expression)
  const value = await cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, sessionId)
  if (value.exceptionDetails) throw new Error(JSON.stringify(value.exceptionDetails))
  return value.result.value
}
try {
  save('start-core')
  result.background_sha256 = sha256(fs.readFileSync(path.join(extension, 'background.js')))
  engine = spawn(path.join(resources, 'HLSDownloaderEngine.exe'), [], { cwd: resources, windowsHide: true, stdio: 'ignore' })
  await waitFor(async () => { try { return await core({ type: 'snapshot' }) } catch { return false } }, 'Core ready')
  await core({ type: 'store_settings', values: { legal_terms_accepted: true, download_dir: path.join(work, 'output'), download_progress_window_enabled: false, download_complete_popup_enabled: false, av_scan_enabled: false } })
  const manifest = path.join(work, 'host.json')
  const hostManifest = { name: 'com.ciaooo55.hls_downloader', description: 'Current source Sandbox verification', path: offlineOnly ? path.join(work, 'UnavailableNativeHost.exe') : process.env.HLS_REPLY_PROXY_PATH, type: 'stdio', ...(firefox ? { allowed_extensions: [extensionId] } : { allowed_origins: [`chrome-extension://${extensionId}/`] }) }
  fs.writeFileSync(manifest, JSON.stringify(hostManifest))
  const hostKey = firefox ? 'HKCU\\Software\\Mozilla\\NativeMessagingHosts\\com.ciaooo55.hls_downloader' : 'HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\com.ciaooo55.hls_downloader'
  const registration = spawnSync('reg.exe', ['add', hostKey, '/ve', '/t', 'REG_SZ', '/d', manifest, '/f'], { windowsHide: true, encoding: 'utf8' })
  assert.equal(registration.status, 0, registration.stderr)
  let inspector
  async function startBrowser() {
  if (firefox) {
    result.scope = 'Actual Firefox + current extension/Core/Host; Marionette protocol, no desktop input'
    save('start-firefox')
    firefoxSession = await startFirefox({ work, extension, extensionId, offlineOnly, result })
    inspector = firefoxSession.inspector
  } else {
  save('start-edge')
  const profile = path.join(work, offlineOnly ? 'edge-offline-profile' : 'edge-profile')
  fs.mkdirSync(profile, { recursive: true })
  const activePort = path.join(profile, 'DevToolsActivePort')
  fs.rmSync(activePort, { force: true })
  browser = spawn('C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe', [`--user-data-dir=${profile}`, '--remote-debugging-port=0', `--load-extension=${extension}`, ...(process.env.HLS_BROWSER_HEADFUL === '1' ? [] : ['--headless=new']), '--no-first-run', '--no-default-browser-check', '--disable-gpu', 'about:blank'], { windowsHide: true, stdio: 'ignore' })
  browser.on('error', error => { result.browser_spawn_error = error.message })
  await waitFor(() => fs.existsSync(activePort), 'Edge debugging endpoint')
  const [port, endpoint] = fs.readFileSync(activePort, 'utf8').trim().split(/\r?\n/)
  result.browser_starts ??= []
  result.browser_starts.push({ pid: browser.pid, port, endpoint, started_at: new Date().toISOString() })
  ws = new WebSocket(`ws://127.0.0.1:${port}${endpoint}`)
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }) })
  ws.addEventListener('message', event => {
    const message = JSON.parse(event.data)
    if (message.id) {
      const entry = pending.get(message.id)
      if (!entry) return
      pending.delete(message.id); clearTimeout(entry.timer)
      if (message.error) entry.reject(new Error(JSON.stringify(message.error)))
      else entry.resolve(message.result)
    } else if (message.method === 'ServiceWorker.workerVersionUpdated' || message.method === 'Runtime.exceptionThrown') {
      (result.worker_diagnostics ??= []).push({method:message.method,params:message.params})
    } else if (message.method === 'Browser.downloadWillBegin') {
      result.browser_events.push(message.params)
      save(result.phase)
    }
  })
  result.browser_version = await cdp('Browser.getVersion')
  await cdp('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: path.join(work, 'browser-downloads'), eventsEnabled: true })
  result.initial_targets = (await cdp('Target.getTargets')).targetInfos
  const popup = await cdp('Target.createTarget', { url: `chrome-extension://${extensionId}/popup.html` })
  inspector = (await cdp('Target.attachToTarget', { targetId: popup.targetId, flatten: true })).sessionId
  await cdp('Runtime.enable', {}, inspector)
  await cdp('ServiceWorker.enable', {}, inspector)
  await waitFor(() => evaluate(inspector, 'typeof chrome === "object" && !!chrome.runtime?.id'), 'Extension loaded')
  assert.equal(await evaluate(inspector, 'chrome.runtime.id'), extensionId)
  save('wait-extension-worker')
  await waitFor(async () => {
    result.ready_targets = (await cdp('Target.getTargets')).targetInfos
    return result.ready_targets.some(target => target.type === 'service_worker' && target.url === `chrome-extension://${extensionId}/background.js`)
  }, 'Current extension service worker registered', 30000)
  }
  }
  await startBrowser()
  assert.equal(await evaluate(inspector, 'chrome.runtime.id'), extensionId)
  save('initial-extension-ping')
  result.ping = await evaluate(inspector, 'chrome.runtime.sendMessage({type:"ping"})')
  assert.equal(result.ping.ok, !offlineOnly, JSON.stringify(result.ping))
  await evaluate(inspector, '(()=>{window.downloadEvents=[];chrome.downloads.onCreated.addListener(item=>downloadEvents.push({id:item.id,url:item.url}));return true})()')
  check(offlineOnly ? 'initial-native-host-unavailable' : 'actual-native-host-ping', result.ping)

  const payload = Buffer.from(Array.from({ length: 256 }, (_, n) => n))
  const body = Buffer.concat(Array(4096).fill(payload))
  server = http.createServer((request, response) => {
    const record = { method: request.method, url: request.url, range: request.headers.range ?? null, bytes: 0 }
    result.server_requests.push(record)
    if (request.url === '/favicon.ico') { response.writeHead(204); response.end(); return }
    if (request.url === '/page') { response.writeHead(200, { 'Content-Type': 'text/html' }); response.end('<!doctype html><title>Current source takeover fixture</title>'); return }
    if (request.url?.startsWith('/media/')) {
      const name = request.url.slice('/media/'.length)
      if (!/^video\.m3u8$|^segment-\d+\.ts$/.test(name)) { response.writeHead(404); response.end(); return }
      const media = fs.readFileSync(path.join('C:\\QAWork\\media\\fixtures', name))
      response.writeHead(200, { 'Content-Type': name.endsWith('.m3u8') ? 'application/vnd.apple.mpegurl' : 'video/mp2t', 'Content-Length': media.length, ...(name.endsWith('.m3u8') ? { 'Content-Disposition': 'attachment; filename="video.m3u8"' } : {}) })
      response.end(media); record.bytes=media.length; return
    }
    if (request.url === '/redirect') { response.writeHead(302, { Location: '/redirect-target', 'Content-Length': '0' }); response.end(); return }
    if (request.url === '/opaque' && result.server_requests.filter(r => r.url === '/opaque').length > 1) {
      response.writeHead(410); response.end(); return
    }
    if (request.method === 'POST') {
      const chunks = []
      request.on('data', chunk => chunks.push(chunk))
      request.on('end', () => {
        const submitted = Buffer.concat(chunks)
        record.request_body_sha256 = sha256(submitted)
        if (submitted.toString() !== 'token=once' || result.server_requests.filter(r => r.url === '/once-post').length !== 1) {
          response.writeHead(410); response.end(); return
        }
        response.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Disposition': 'attachment; filename="post.bin"', 'Content-Length': body.length })
        response.end(body); record.bytes = body.length
      })
      return
    }
    let start = 0, end = body.length - 1
    if (request.headers.range) {
      const range = /^bytes=(\d+)-(\d*)$/.exec(request.headers.range)
      if (!range) { response.writeHead(416); response.end(); return }
      start = Number(range[1]); if (range[2]) end = Math.min(end, Number(range[2]))
    }
    response.writeHead(request.headers.range ? 206 : 200, { 'Content-Type': 'application/octet-stream', 'Content-Disposition': 'attachment; filename="current.bin"', 'Content-Length': end - start + 1, 'Accept-Ranges': 'bytes', ...(request.headers.range ? { 'Content-Range': `bytes ${start}-${end}/${body.length}` } : {}) })
    let offset = start
    const timer = setInterval(() => {
      if (offset > end) { clearInterval(timer); response.end(); return }
      const next = Math.min(offset + 8192, end + 1)
      response.write(body.subarray(offset, next)); record.bytes += next - offset; offset = next
    }, request.url === '/slow-progress' ? 50 : 10)
    response.on('close', () => clearInterval(timer))
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  const base = `http://127.0.0.1:${server.address().port}`
  let pageSession
  if (firefox) pageSession = await firefoxSession.newPage(base + '/page')
  else {
    const page = await cdp('Target.createTarget', { url: base + '/page' })
    pageSession = (await cdp('Target.attachToTarget', { targetId: page.targetId, flatten: true })).sessionId
    await cdp('Page.enable', {}, pageSession)
  }
  await waitFor(() => evaluate(pageSession, 'document.readyState === "complete"'), 'Fixture page')
  if (!offlineOnly) {
  const before = new Set((await core({ type: 'snapshot' })).tasks.map(t => t.task_id))
  const oldOffers = new Set((await core({ type: 'load_handoffs' })).items.map(row => JSON.parse(row).id))
  save('response-navigation')
  // Navigation, rather than extension download-now, exercises actual response interception.
  await evaluate(pageSession, `(location.assign(${JSON.stringify(base + '/opaque')}),true)`)
  const handoff = await waitFor(async () => {
    const offers = (await core({ type: 'load_handoffs' })).items.map(JSON.parse).filter(row => !oldOffers.has(row.id))
    assert.ok(offers.length <= 1, 'Duplicate handoffs')
    return offers[0]
  }, 'One persisted handoff', 40000)
  result.persisted_handoff = handoff
  assert.equal(handoff.status, 'pending')
  assert.deepEqual((await core({ type: 'snapshot' })).tasks.map(t => t.task_id).filter(id => !before.has(id)), [])
  await core({ type: 'command', command: { kind: 'accept_handoff', handoff_id: handoff.id, filename: 'current.bin', download_dir: path.join(work, 'output'), trusted_ui: true } })
  const task = await waitFor(async () => {
    const tasks = (await core({ type: 'snapshot' })).tasks.filter(t => !before.has(t.task_id))
    assert.ok(tasks.length <= 1, 'Duplicate tasks')
    assert.ok(!tasks.some(t => t.status === 'failed'), JSON.stringify(tasks))
    return tasks[0]?.status === 'completed' ? tasks[0] : null
  }, 'One completed Core task', 60000)
  const outputHash = sha256(fs.readFileSync(task.output_path))
  assert.equal(outputHash, sha256(body))
  assert.equal(result.server_requests.filter(r => r.url === '/opaque').length, 1, 'One-use GET must not be replayed')
  result.task = task
  result.output_sha256 = outputHash
  await waitFor(async () => {
    result.browser_items = await evaluate(inspector, 'chrome.downloads.search({})')
    return result.browser_items.length === 0
  }, 'Browser transfer released', 15000)
  result.extension_download_events = await evaluate(inspector, 'downloadEvents')
  result.zero_download_item = result.browser_events.length === 0 && result.extension_download_events.length === 0
  check('one-use-get-single-owner-accepted-output-hash', { tasks: 1, handoffs: 1, requests: 1, sha256: outputHash, zero_download_item: result.zero_download_item })
  const offers = fs.readFileSync(path.join(process.env.HLS_REPLY_PROXY_STATE, 'offers.jsonl'), 'utf8').trim().split(/\r?\n/).map(JSON.parse)
  assert.equal(offers.length, 2)
  assert.deepEqual(offers.map(row => row.injected), [true, false])
  assert.equal(new Set(offers.map(row => row.client_request_id)).size, 1)
  assert.equal(new Set(offers.map(row => row.request_sha256)).size, 1)
  assert.ok(offers.every(row => row.handoff_id === handoff.id))
  check('lost-native-reply-same-request-single-admission', offers)

  async function newOffer(trigger) {
    const known = new Set((await core({ type: 'load_handoffs' })).items.map(row => JSON.parse(row).id))
    await evaluate(pageSession, trigger)
    return waitFor(async () => {
      const found = (await core({ type: 'load_handoffs' })).items.map(JSON.parse).filter(row => !known.has(row.id))
      assert.ok(found.length <= 1, 'Duplicate scenario handoffs')
      return found[0]
    }, 'Scenario handoff persisted', 40000)
  }
  const retainedBrowserIds = new Set()
  async function noBrowserItems(timeout = 20000) {
    const started = Date.now()
    let lastRecorded = 0
    await waitFor(async () => {
      const items = await evaluate(inspector, 'chrome.downloads.search({})')
      const records = await evaluate(inspector, 'chrome.storage.local.get("local-browser-download-handoffs")')
      result.cleanup_observations ??= []
      const released = items.every(item => retainedBrowserIds.has(item.id))
      if (Date.now()-lastRecorded >= 1000 || released) {
        lastRecorded = Date.now()
        if (result.cleanup_observations.length < 240) result.cleanup_observations.push({ elapsed_ms: Date.now()-started, items: items.map(item=>({id:item.id,state:item.state,exists:item.exists})), records })
        save(result.phase)
      }
      return released
    }, 'Browser transfer released', timeout)
    return Date.now()-started
  }
  async function acceptImported(offer, filename, recovery = false) {
    const known = new Set((await core({ type: 'snapshot' })).tasks.map(t => t.task_id))
    await core({ type: 'command', command: { kind: 'accept_handoff', handoff_id: offer.id, filename, download_dir: path.join(work, 'output'), trusted_ui: true } })
    const imported = await waitFor(async () => {
      const tasks = (await core({ type: 'snapshot' })).tasks.filter(t => !known.has(t.task_id))
      assert.ok(tasks.length <= 1, 'Duplicate import tasks')
      assert.ok(!tasks.some(t => t.status === 'failed'), JSON.stringify(tasks))
      return tasks[0]?.status === 'completed' ? tasks[0] : null
    }, 'One imported task completed')
    const hash = sha256(fs.readFileSync(imported.output_path))
    assert.equal(hash, sha256(body))
    const cleanup_ms = await noBrowserItems(recovery ? 90000 : 20000)
    return { task: imported, sha256: hash, cleanup_ms }
  }

  save('response-rejected')
  const tasksBeforeReject = (await core({ type: 'snapshot' })).tasks.map(t => t.task_id)
  const rejected = await newOffer(`(location.assign(${JSON.stringify(base + '/reject')}),true)`)
  await core({ type: 'command', command: { kind: 'reject_handoff', handoff_id: rejected.id } })
  const rejectedFile = (await evaluate(inspector, 'chrome.downloads.search({})')).find(item => item.url === base + '/reject')
  assert.ok(rejectedFile && rejectedFile.state === 'complete', 'Refusal must retain original browser bytes')
  assert.equal(sha256(fs.readFileSync(rejectedFile.filename)), sha256(body))
  retainedBrowserIds.add(rejectedFile.id)
  assert.deepEqual((await core({ type: 'snapshot' })).tasks.map(t => t.task_id), tasksBeforeReject)
  assert.equal(result.server_requests.filter(r => r.url === '/reject').length, 1)
  check('response-rejection-retains-original-no-task-no-second-request', { handoff_id: rejected.id, server_requests: 1, source_sha256: sha256(body) })

  save('one-time-post-import')
  const postOffer = await newOffer(`(()=>{const f=document.createElement('form');f.method='POST';f.action=${JSON.stringify(base + '/once-post')};const i=document.createElement('input');i.name='token';i.value='once';f.append(i);document.body.append(f);f.submit();return true})()`)
  const post = await acceptImported(postOffer, 'post.bin')
  assert.equal(result.server_requests.filter(r => r.url === '/once-post').length, 1)
  check('one-time-post-original-bytes-imported-once', post)

  save('blob-import')
  const requestsBeforeBlob = result.server_requests.length
  const blobOffer = await newOffer(`(()=>{const b=new Blob([Uint8Array.from({length:1048576},(_,i)=>i%256)],{type:'application/octet-stream'});const a=document.createElement('a');a.href=URL.createObjectURL(b);a.download='blob.bin';document.body.append(a);a.click();return true})()`)
  const blob = await acceptImported(blobOffer, 'blob.bin')
  assert.equal(result.server_requests.length, requestsBeforeBlob)
  check('blob-original-bytes-imported-without-network', blob)

  save('redirect-import')
  const redirectOffer = await newOffer(`(location.assign(${JSON.stringify(base + '/redirect')}),true)`)
  const redirectImport = await acceptImported(redirectOffer, 'redirect.bin')
  assert.equal(result.server_requests.filter(r => r.url === '/redirect').length, 1)
  assert.equal(result.server_requests.filter(r => r.url === '/redirect-target').length, 1)
  check('redirect-original-response-single-owner-sha256', redirectImport)

  save('adaptive-manifest-is-media-not-completed-file')
  const mediaKnown = new Set((await core({type:'snapshot'})).tasks.map(t=>t.task_id))
  const mediaOffer = await newOffer(`(location.assign(${JSON.stringify(base + '/media/video.m3u8')}),true)`)
  assert.equal(mediaOffer.offer.resource_kind, 'hls', JSON.stringify(mediaOffer))
  await core({type:'command',command:{kind:'accept_handoff',handoff_id:mediaOffer.id,filename:'manifest-media.mp4',download_dir:path.join(work,'output'),trusted_ui:true}})
  const mediaTask = await waitFor(async()=>{
    const found=(await core({type:'snapshot'})).tasks.filter(t=>!mediaKnown.has(t.task_id))
    assert.ok(found.length<=1);assert.ok(!found.some(t=>t.status==='failed'),JSON.stringify(found))
    return found[0]?.status==='completed'?found[0]:null
  },'Manifest downloaded as complete media',60000)
  assert.equal(mediaTask.resource_kind,'hls')
  const decodeHash = file => {
    const proc = spawnSync(path.join(resources,'ffmpeg.exe'),['-nostdin','-v','error','-i',file,'-map','0:v:0','-c:v','rawvideo','-pix_fmt','yuv420p','-f','hash','-hash','sha256','-'],{windowsHide:true,encoding:'utf8',timeout:30000})
    assert.equal(proc.status,0,proc.stderr);return proc.stdout.trim()
  }
  assert.equal(decodeHash(mediaTask.output_path),decodeHash('C:\\QAWork\\media\\fixtures\\original.mp4'))
  await noBrowserItems()
  check('adaptive-manifest-produces-hls-video-with-correct-decoded-frames', {task:mediaTask,sha256:sha256(fs.readFileSync(mediaTask.output_path))})

  save('browser-receiving-progress')
  const progressKnown = new Set((await core({type:'load_handoffs'})).items.map(row=>JSON.parse(row).id))
  const sourceTabId = await evaluate(inspector, `chrome.tabs.query({}).then(tabs=>tabs.find(t=>t.url===${JSON.stringify(base + '/page')})?.id)`)
  assert.ok(Number.isInteger(sourceTabId), 'Source page tab ID')
  await evaluate(pageSession, `(location.assign(${JSON.stringify(base + '/slow-progress')}),true)`)
  const projection = await waitFor(async()=>{
    const rows = await evaluate(inspector, `chrome.tabs.get(${sourceTabId}).then(async tab=>{await chrome.windows.update(tab.windowId,{focused:true});await chrome.tabs.update(tab.id,{active:true});await chrome.action.openPopup({windowId:tab.windowId});await new Promise(resolve=>setTimeout(resolve,150));const view=chrome.extension.getViews({type:'popup'})[0];if(!view)throw new Error('Actual action popup not found');return view.chrome.runtime.sendMessage({type:'local-download-progress',tabId:${sourceTabId}})})`)
    result.progress_diagnostics ??= []
    if(result.progress_diagnostics.length < 40) result.progress_diagnostics.push({rows,sourceTabId,items:await evaluate(inspector,'chrome.downloads.search({})'),records:await evaluate(inspector,'chrome.storage.local.get("local-browser-download-handoffs")')})
    save('browser-receiving-progress')
    return rows?.find(row=>row.state==='receiving' && row.bytesReceived>0 && row.bytesReceived<row.totalBytes)
  },'Real browser receiving progress',12000)
  assert.deepEqual(Object.keys(projection).sort(),['bytesReceived','filename','id','state','totalBytes'])
  const progressOffer = await waitFor(async()=>{
    const found=(await core({type:'load_handoffs'})).items.map(JSON.parse).filter(row=>!progressKnown.has(row.id))
    assert.ok(found.length<=1);return found[0]
  },'Receiving transfer ready for import',30000)
  await evaluate(inspector, 'chrome.extension.getViews({type:"popup"})[0]?.close()')
  const progressImport = await acceptImported(progressOffer,'progress.bin')
  assert.equal(result.server_requests.filter(r=>r.url==='/slow-progress').length,1)
  check('real-receiving-bytes-projected-to-source-tab-and-imported', {projection,...progressImport})
  result.download_item_events_before_restart = await evaluate(inspector,'downloadEvents')

  save('pending-local-import-extension-reload')
  const reloadOffer = await newOffer(`(location.assign(${JSON.stringify(base + '/reload')}),true)`)
  result.reload_before = { items: await evaluate(inspector, 'chrome.downloads.search({})'), records: await evaluate(inspector, 'chrome.storage.local.get("local-browser-download-handoffs")') }
  const beforeReloadWorkers = firefox ? [] : (await cdp('Target.getTargets')).targetInfos.filter(target => target.type === 'service_worker' && target.url.startsWith(`chrome-extension://${extensionId}/`)).map(target => target.targetId)
  result.before_reload_worker_ids = beforeReloadWorkers
  await evaluate(inspector, '(setTimeout(()=>chrome.runtime.reload(),100),true)')
  await delay(1000)
  if (firefox) {
    await firefoxSession.command('WebDriver:SwitchToWindow', { handle: pageSession })
    inspector = await firefoxSession.newPage(`moz-extension://b64ca36e-6620-433a-83db-43a7bec7092e/popup.html`)
  }
  else {
    result.reloaded_worker = await waitFor(async () => (await cdp('Target.getTargets')).targetInfos.find(target => target.type === 'service_worker' && target.url.startsWith(`chrome-extension://${extensionId}/`) && !beforeReloadWorkers.includes(target.targetId)), 'New reloaded extension worker ready', 60000)
    const popup = await cdp('Target.createTarget', { url: `chrome-extension://${extensionId}/popup.html` })
    inspector = (await cdp('Target.attachToTarget', { targetId: popup.targetId, flatten: true })).sessionId
  }
  await waitFor(async () => {
    result.reload_document = await evaluate(inspector, '({url:location.href,ready:document.readyState,title:document.title,runtimeId:globalThis.chrome?.runtime?.id ?? null,body:document.body?.innerText?.slice(0,300)})')
    save('pending-local-import-extension-reload')
    return result.reload_document.runtimeId
  }, 'Reloaded extension document ready')
  await evaluate(inspector, '(()=>{window.qaReloadPing={pending:true};chrome.runtime.sendMessage({type:"ping"}).then(reply=>window.qaReloadPing={reply},error=>window.qaReloadPing={error:String(error)});return true})()')
  await waitFor(async () => {
    result.reload_ping = await evaluate(inspector, 'window.qaReloadPing')
    save('pending-local-import-extension-reload')
    assert.ok(!result.reload_ping?.error, JSON.stringify(result.reload_ping))
    if (result.reload_ping?.reply) assert.equal(result.reload_ping.reply.ok, true, JSON.stringify(result.reload_ping))
    return result.reload_ping?.reply?.ok
  }, 'Reloaded extension connected', 65000)
  const reloadedRecords = await evaluate(inspector, 'chrome.storage.local.get("local-browser-download-handoffs")')
  result.reload_after = { items: await evaluate(inspector, 'chrome.downloads.search({})'), records: reloadedRecords }
  assert.ok(reloadedRecords['local-browser-download-handoffs'].some(row => row.requestId === reloadOffer.request_id))
  const reloaded = await acceptImported(reloadOffer, 'reloaded.bin', true)
  assert.equal(result.server_requests.filter(r => r.url === '/reload').length, 1)
  check('extension-reload-same-pending-import-single-task-hash', reloaded)

  save('pending-local-import-browser-restart')
  const restartOffer = await newOffer(`(location.assign(${JSON.stringify(base + '/restart')}),true)`)
  if (firefox) await firefoxSession.close()
  else {
    await cdp('Browser.close')
    ws.close()
    await waitFor(() => browser.exitCode !== null, 'Browser exited')
  }
  await startBrowser()
  save('restarted-extension-ping')
  // Observe the bounded NativeBridge request separately from CDP's command
  // lifetime, so a pending native reply is distinguishable from a dead page.
  await evaluate(inspector, '(()=>{window.qaRestartPing={pending:true};chrome.runtime.sendMessage({type:"ping"}).then(reply=>window.qaRestartPing={reply},error=>window.qaRestartPing={error:String(error)});return true})()')
  await waitFor(async () => {
    result.restart_ping = await evaluate(inspector, 'window.qaRestartPing')
    save('restarted-extension-ping')
    assert.ok(!result.restart_ping?.error, JSON.stringify(result.restart_ping))
    if (result.restart_ping?.reply) assert.equal(result.restart_ping.reply.ok, true, JSON.stringify(result.restart_ping))
    return result.restart_ping?.reply?.ok
  }, 'Restarted extension connected', 65000)
  const restartedRecords = await evaluate(inspector, 'chrome.storage.local.get("local-browser-download-handoffs")')
  result.restart_after = { items: await evaluate(inspector, 'chrome.downloads.search({})'), records: restartedRecords }
  assert.ok(restartedRecords['local-browser-download-handoffs']?.some(row => row.requestId === restartOffer.request_id), 'Original import identity must survive browser restart')
  const restarted = await acceptImported(restartOffer, 'restarted.bin', true)
  await waitFor(async () => {
    const stored = await evaluate(inspector, 'chrome.storage.local.get("local-browser-download-handoffs")')
    return !stored['local-browser-download-handoffs']?.some(row => row.requestId === restartOffer.request_id)
  }, 'Completed restarted import tracker released', 70000)
  assert.equal(result.server_requests.filter(r => r.url === '/restart').length, 1)
  const source = restartOffer.offer.local_source_path
  const retainedSource = fs.existsSync(source)
  if (retainedSource) assert.equal(sha256(fs.readFileSync(source)), sha256(body))
  check('browser-restart-same-pending-import-single-task-hash', { ...restarted, tracker_released: true, source_retained: retainedSource, temporary_firefox_addon_reinstalled: firefox })
  } else {
  save('initial-native-host-offline')
  const offlineInspector = inspector
  const knownOffers = (await core({ type: 'load_handoffs' })).items.map(row => JSON.parse(row).id)
  const knownTasks = (await core({ type: 'snapshot' })).tasks.map(t => t.task_id)
  const previousDownloadIds = new Set((await evaluate(offlineInspector, 'chrome.downloads.search({})')).map(item => item.id))
  save('offline-navigate')
  await evaluate(pageSession, `(location.assign(${JSON.stringify(base + '/offline')}),true)`)
  const browserFile = await waitFor(async () => {
    const items = await evaluate(offlineInspector, 'chrome.downloads.search({})')
    result.offline_browser_items = items
    const found = items.filter(item => !previousDownloadIds.has(item.id))
    assert.ok(found.length <= 1)
    return found[0]?.state === 'complete' ? found[0] : null
  }, 'Offline original browser response preserved', 40000)
  assert.equal(sha256(fs.readFileSync(browserFile.filename)), sha256(body))
  assert.equal(result.server_requests.filter(r => r.url === '/offline').length, 1)
  assert.deepEqual((await core({ type: 'load_handoffs' })).items.map(row => JSON.parse(row).id), knownOffers)
  assert.deepEqual((await core({ type: 'snapshot' })).tasks.map(t => t.task_id), knownTasks)
  check('initial-offline-original-browser-response-preserved', { file: browserFile, sha256: sha256(body), server_requests: 1, no_new_core_handoff: true, no_new_core_task: true, baseline_tasks: knownTasks.length, ping: result.ping })
  }
  // Correctness of transfer and the strict no-DownloadItem requirement are separate results.
  result.passed = true
  result.strict_zero_item_passed = result.zero_download_item
} catch (error) {
  result.error = error.stack ?? String(error)
  if (ws?.readyState === WebSocket.OPEN) {
    try { result.failure_targets = (await cdp('Target.getTargets')).targetInfos } catch {}
  }
  process.exitCode = 1
} finally {
  if (firefoxSession) await firefoxSession.close()
  if (ws?.readyState === WebSocket.OPEN) { try { await cdp('Browser.close') } catch {} }
  ws?.close()
  for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(new Error('Probe closed')) }
  pending.clear()
  if (browser?.pid) spawnSync('taskkill.exe', ['/PID', String(browser.pid), '/T', '/F'], { windowsHide: true })
  try { await core({ type: 'command', command: { kind: 'shutdown' } }) } catch {}
  if (engine?.pid) engine.kill()
  if (server) { server.closeAllConnections(); server.close() }
  result.finished = true
  save(result.phase)
}
