const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const http = require('node:http');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');
const assert = require('node:assert/strict');
assert.equal(process.env.USERNAME, 'WDAGUtilityAccount');
const input = 'C:/QAInput', reports = 'C:/QAReports', install = 'C:/QAInstalled';
const config = JSON.parse(fs.readFileSync(input + '/installer-expectations.json', 'utf8').replace(/^\uFEFF/, ''));
const result = { passed: false, finished: false, checks: [], error: '', source_commit: config.source_commit };
const save = () => fs.writeFileSync(reports + '/installer-qa.json', JSON.stringify(result, null, 2));
const hash = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function wait(check, label, timeout = 60000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) { try { const value = await check(); if (value) return value; } catch {} await pause(100); }
  throw new Error(label);
}
function run(exe, args, log) {
  return new Promise((resolve, reject) => {
    const child = spawn(exe, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    const timer = setTimeout(() => { child.kill(); reject(new Error('process timed out: ' + exe)); }, 180000);
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { output += chunk; });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('exit', code => { clearTimeout(timer); if (log) fs.writeFileSync(reports + '/' + log, output); code === 0 ? resolve(output) : reject(new Error(exe + ' exited ' + code + ': ' + output)); });
  });
}
function ipc(message) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection('\\\\.\\pipe\\HLSDownloader.v7');
    let buffer = Buffer.alloc(0);
    socket.setTimeout(10000, () => socket.destroy(new Error('Core IPC timeout')));
    socket.once('error', reject);
    socket.once('connect', () => {
      const body = Buffer.from(JSON.stringify(message)), head = Buffer.alloc(4);
      head.writeUInt32LE(body.length); socket.write(Buffer.concat([head, body]));
    });
    socket.on('data', chunk => {
      buffer = Buffer.concat([buffer, chunk]);
      if (buffer.length < 4) return;
      const size = buffer.readUInt32LE(0);
      if (size > 4 * 1024 * 1024) return socket.destroy(new Error('invalid IPC size'));
      if (buffer.length < size + 4) return;
      socket.end();
      try { const reply = JSON.parse(buffer.subarray(4, size + 4)); if (reply.type === 'error') reject(new Error(JSON.stringify(reply))); else resolve(reply); } catch (error) { reject(error); }
    });
  });
}
let sequence = 1;
const request = body => ipc({ request_id: sequence++, ...body });
const command = body => request({ type: 'command', command: body });
const check = (name, value) => { result.checks.push({ name, actual: value }); save(); };
const engine = () => spawn(install + '/app/resources/HLSDownloaderEngine.exe', [], { cwd: install, windowsHide: true, stdio: 'ignore' });
async function productState(code) {
  const script = 'Add-Type -Namespace QA -Name MSI -MemberDefinition \'[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);\'; [QA.MSI]::MsiQueryProductState(\'' + code + '\')';
  return Number((await run('powershell.exe', ['-NoProfile', '-Command', script])).trim());
}
const payload = Buffer.from('verified redirect download\n');
const events = [];
const server = http.createServer((req, res) => {
  const chunks = [];
  req.on('data', chunk => chunks.push(chunk));
  req.on('end', () => {
    events.push({ method: req.method, url: req.url, body: Buffer.concat(chunks).toString(), content_type: req.headers['content-type'] || '' });
    if (req.url === '/export') { res.writeHead(303, { Location: '/file', 'Content-Length': '0' }); res.end(); }
    else { res.writeHead(200, { 'Content-Length': payload.length, 'Content-Type': 'application/octet-stream' }); res.end(payload); }
  });
});
async function create(url, filename, method) {
  const created = await command({ kind: 'create_task', spec: { url, resource_kind: 'file', request_method: method, filename, title: filename, download_dir: 'C:/QAWork/downloads', concurrency: 1 } });
  const id = created.events.find(event => event.event.kind === 'task_created').event.snapshot.task_id;
  await command({ kind: 'task_action', task_id: id, action: 'start' });
  const task = await wait(async () => {
    const task = (await request({ type: 'snapshot' })).tasks.find(task => task.task_id === id);
    if (task.status === 'failed') throw new Error('task failed: ' + JSON.stringify(task));
    return task.status === 'completed' ? task : false;
  }, 'download did not complete');
  assert.deepEqual(fs.readFileSync(task.output_path), payload);
  return task;
}
(async () => {
  save();
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:' + server.address().port;
  try {
    fs.copyFileSync(input + '/baseline.msi', 'C:/QAWork/baseline.msi');
    fs.copyFileSync('C:/QADelivery/HLSDownloader-7.0.3-20261026-未签名.msi', 'C:/QAWork/candidate.msi');
    await run('msiexec.exe', ['/i', 'C:\\QAWork\\baseline.msi', '/qn', 'INSTALLDIR=C:\\QAInstalled', '/l*v', 'C:\\QAReports\\baseline-install.log']);
    assert.equal(await productState(config.old_product), 5);
    check('baseline installed', config.old_product);
    engine();
    await wait(async () => (await request({ type: 'snapshot' })).type === 'snapshot', 'baseline Core did not start');
    await request({ type: 'store_setting', key: 'legal_terms_accepted', value: true });
    await request({ type: 'store_setting', key: 'download_dir', value: 'C:\\QAWork\\downloads' });
    let settings = await request({ type: 'store_setting', key: 'download_speed_limit_kib', value: 137 });
    assert.equal(settings.speed_limit_kib, 137);
    const baselineTask = await create(origin + '/file', 'baseline.bin', 'GET');
    check('baseline task and setting saved', { task_id: baselineTask.task_id, file_sha256: hash(baselineTask.output_path), speed_limit_kib: settings.speed_limit_kib });
    await run('msiexec.exe', ['/i', 'C:\\QAWork\\candidate.msi', '/qn', '/l*v', 'C:\\QAReports\\candidate-upgrade.log']);
    assert.equal(await productState(config.old_product), -1);
    assert.equal(await productState(config.new_product), 5);
    const actualHashes = {};
    for (const [relative, expected] of Object.entries(config.installed_hashes)) { actualHashes[relative] = hash(install + '/' + relative); assert.equal(actualHashes[relative], expected, relative); }
    check('running-Core upgrade and installed payload identities', actualHashes);
    engine();
    await wait(async () => (await request({ type: 'snapshot' })).type === 'snapshot', 'new Core did not start');
    settings = await request({ type: 'load_settings' });
    assert.equal(settings.speed_limit_kib, 137);
    const persisted = (await request({ type: 'snapshot' })).tasks.find(task => task.task_id === baselineTask.task_id);
    assert.equal(persisted.status, 'completed'); assert.deepEqual(fs.readFileSync(persisted.output_path), payload);
    check('task file and setting preserved', true);
    const before = events.length;
    const exported = await create(origin + '/export', 'exported.bin', 'POST');
    const exportedRequests = events.slice(before);
    assert.deepEqual(exportedRequests.map(event => [event.method, event.url]), [['POST', '/export'], ['GET', '/file']]);
    assert.equal(exportedRequests[1].body, ''); assert.equal(exportedRequests[1].content_type, '');
    check('installed Core completes POST then 303 then GET', { task_id: exported.task_id, requests: exportedRequests, sha256: hash(exported.output_path) });
    for (const task of [persisted, exported]) {
      await command({ kind: 'task_action', task_id: task.task_id, action: 'delete_files' });
      assert.equal(fs.existsSync(task.output_path), false);
    }
    assert.equal((await request({ type: 'snapshot' })).tasks.length, 0);
    check('sandbox canary tasks and files deleted', true);
    await run('msiexec.exe', ['/x', config.new_product, '/qn', '/l*v', 'C:\\QAReports\\candidate-uninstall.log']);
    assert.equal(await productState(config.new_product), -1);
    assert.equal(fs.existsSync(install + '/app/resources/HLSDownloaderEngine.exe'), false);
    check('uninstall removed product and executable', true);
    result.passed = true;
  } catch (error) { result.error = error.stack || String(error); process.exitCode = 1; }
  finally { server.close(); result.finished = true; save(); }
})();
