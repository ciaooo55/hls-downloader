// Sandbox-only Marionette client: length-prefixed JSON, no driver dependency.
// https://firefox-source-docs.mozilla.org/remote/marionette/Protocol.html
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import net from 'node:net'
import { spawn, spawnSync } from 'node:child_process'
import { setTimeout as delay } from 'node:timers/promises'

export async function startFirefox({ work, extension, extensionId, offlineOnly, result }) {
  assert.equal(process.env.USERNAME, 'WDAGUtilityAccount')
  const profile = path.join(work, offlineOnly ? 'firefox-offline-profile' : 'firefox-profile')
  const downloads = path.join(work, 'browser-downloads')
  fs.mkdirSync(profile, { recursive: true })
  fs.mkdirSync(downloads, { recursive: true })
  const uuid = 'b64ca36e-6620-433a-83db-43a7bec7092e'
  const prefs = {
    'marionette.port': 2828,
    'browser.shell.checkDefaultBrowser': false,
    'browser.download.folderList': 2,
    'browser.download.dir': downloads,
    'browser.download.useDownloadDir': true,
    'browser.helperApps.neverAsk.saveToDisk': 'application/octet-stream',
    'browser.download.always_ask_before_handling_new_types': false,
    'extensions.webextensions.uuids': JSON.stringify({ [extensionId]: uuid }),
  }
  fs.writeFileSync(path.join(profile, 'user.js'), Object.entries(prefs).map(([key, value]) => `user_pref(${JSON.stringify(key)}, ${JSON.stringify(value)});`).join('\n'))
  const browser = spawn('C:\\QAFirefox\\firefox.exe', ['-no-remote', '-profile', profile, '--headless', '--marionette', 'about:blank'], { windowsHide: true, stdio: 'ignore', env: { ...process.env, MOZ_REMOTE_ALLOW_SYSTEM_ACCESS: '1' } })
  browser.on('error', error => { result.browser_spawn_error = error.message })
  let socket
  const deadline = Date.now() + 30000
  while (!socket && Date.now() < deadline) {
    socket = await new Promise(resolve => {
      const candidate = net.createConnection(2828, '127.0.0.1')
      candidate.once('connect', () => resolve(candidate))
      candidate.once('error', () => { candidate.destroy(); resolve(null) })
    })
    if (!socket) await delay(100)
  }
  assert.ok(socket, `Firefox Marionette unavailable: ${result.browser_spawn_error || browser.exitCode}`)
  let buffer = Buffer.alloc(0), sequence = 0
  const pending = new Map()
  socket.on('data', data => {
    buffer = Buffer.concat([buffer, data])
    for (;;) {
      const colon = buffer.indexOf(58)
      if (colon < 0) break
      const length = Number(buffer.subarray(0, colon).toString())
      if (buffer.length < colon + 1 + length) break
      const message = JSON.parse(buffer.subarray(colon + 1, colon + 1 + length))
      buffer = buffer.subarray(colon + 1 + length)
      if (!Array.isArray(message)) { result.marionette_protocol = message; continue }
      const entry = pending.get(message[1])
      if (!entry) continue
      pending.delete(message[1]); clearTimeout(entry.timer)
      if (message[2]) entry.reject(new Error(JSON.stringify(message[2])))
      else entry.resolve(message[3])
    }
  })
  socket.on('error', error => { result.marionette_error = error.message })
  function command(name, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++sequence
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Marionette timeout: ${name}`)) }, 20000)
      pending.set(id, { resolve, reject, timer })
      const body = Buffer.from(JSON.stringify([0, id, name, params]))
      socket.write(Buffer.concat([Buffer.from(`${body.length}:`), body]))
    })
  }
  async function newPage(url) {
    const { handle } = await command('WebDriver:NewWindow', { type: 'tab' })
    await command('WebDriver:SwitchToWindow', { handle })
    await command('WebDriver:Navigate', { url })
    return handle
  }
  async function evaluate(handle, expression) {
    await command('WebDriver:SwitchToWindow', { handle })
    const response = await command('WebDriver:ExecuteAsyncScript', {
      script: `const done=arguments[arguments.length-1];try{Promise.resolve(${expression}).then(value=>done({value}),error=>done({failure:String(error)}))}catch(error){done({failure:String(error)})}`,
      args: [], newSandbox: false,
    })
    if (response.value.failure) throw new Error(response.value.failure)
    return response.value.value
  }
  async function close() {
    try { await command('Marionette:Quit', { flags: ['eAttemptQuit'] }) } catch {}
    socket.destroy()
    for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(new Error('Firefox closed')) }
    if (browser.pid) spawnSync('taskkill.exe', ['/PID', String(browser.pid), '/T', '/F'], { windowsHide: true })
  }
  try {
    result.browser_version = await command('WebDriver:NewSession', { capabilities: { alwaysMatch: { acceptInsecureCerts: false } } })
    await command('WebDriver:SetTimeouts', { script: 15000, pageLoad: 15000 })
    assert.equal((await command('Addon:Install', { path: extension, temporary: true })).value, extensionId)
    const inspector = await newPage(`moz-extension://${uuid}/popup.html`)
    return { inspector, newPage, evaluate, command, close }
  } catch (error) {
    await close()
    throw error
  }
}
