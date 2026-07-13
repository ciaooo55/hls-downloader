// ==UserScript==
// @name         m3u8 一键下载
// @namespace    hls-downloader
// @version      4.1.0
// @description  打开网页自动嗅探 m3u8 资源，一键下载，实时进度，完成后可打开文件/目录
// @match        https://*/*
// @exclude      http://127.0.0.1/*
// @exclude      http://localhost/*
// @grant        GM_xmlhttpRequest
// @grant        GM_addStyle
// @grant        GM_setClipboard
// @connect      127.0.0.1
// @run-at       document-idle
// ==/UserScript==

(function () {
  'use strict';

  // ============ Configuration ============
  const API_BASE = 'http://127.0.0.1:8765/api';
  const TOKEN = '55555'; // Must match config.json token
  const SCRIPT_VERSION = '4.1.0';
  // =======================================

  const found = new Map();
  let bannerEl = null;
  const taskStates = new Map(); // taskId -> { status, stage, pct, speed, size, output, error, pollTimer }

  function authHeaders() {
    return { 'Content-Type': 'application/json', 'X-Token': TOKEN };
  }

  function addM3U8(url, source) {
    if (!url || found.has(url)) return;
    if (!url.includes('.m3u8')) return;
    try {
      const u = new URL(url);
      found.set(u.href, { url: u.href, source, time: Date.now() });
    } catch {
      found.set(url, { url, source, time: Date.now() });
    }
    showBanner();
  }

  // Sniff: hook fetch + xhr + performance + video src
  const _fetch = window.fetch;
  window.fetch = function (...a) {
    const u = typeof a[0] === 'string' ? a[0] : a[0]?.url || '';
    if (u.includes('m3u8')) addM3U8(u, 'fetch');
    return _fetch.apply(this, a);
  };
  const _open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (m, u, ...r) {
    if (typeof u === 'string' && u.includes('m3u8')) addM3U8(u, 'xhr');
    return _open.call(this, m, u, ...r);
  };
  const _srcDesc = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'src');
  if (_srcDesc) {
    Object.defineProperty(HTMLMediaElement.prototype, 'src', {
      set(v) { if (v?.includes('m3u8')) addM3U8(v, 'video'); return _srcDesc.set.call(this, v); },
      get() { return _srcDesc.get.call(this); },
    });
  }

  function scan() {
    try {
      for (const e of performance.getEntriesByType('resource')) {
        if (e.name?.includes('m3u8')) addM3U8(e.name, 'perf');
      }
    } catch {}
  }
  setInterval(scan, 3000);
  setTimeout(scan, 1000);
  setTimeout(scan, 3000);
  setTimeout(scan, 6000);

  // ============ API helpers ============
  function apiPost(path, data) {
    return new Promise((resolve, reject) => {
      GM_xmlhttpRequest({
        method: 'POST',
        url: API_BASE + path,
        headers: authHeaders(),
        data: JSON.stringify(data),
        onload: r => {
          try {
            const body = JSON.parse(r.responseText);
            if (r.status < 200 || r.status >= 300) return reject(new Error(body.detail || `HTTP ${r.status}`));
            resolve(body);
          } catch (error) { reject(error); }
        },
        onerror: reject,
      });
    });
  }

  function apiGet(path) {
    return new Promise((resolve, reject) => {
      GM_xmlhttpRequest({
        method: 'GET',
        url: API_BASE + path,
        headers: { 'X-Token': TOKEN },
        onload: r => {
          try {
            const body = JSON.parse(r.responseText);
            if (r.status < 200 || r.status >= 300) return reject(new Error(body.detail || `HTTP ${r.status}`));
            resolve(body);
          } catch (error) { reject(error); }
        },
        onerror: reject,
      });
    });
  }

  function pingDownloader() {
    apiPost('/userscript/ping', {
      version: SCRIPT_VERSION,
      page_url: location.href,
    }).catch(() => {});
  }

  setTimeout(pingDownloader, 500);
  setInterval(pingDownloader, 60000);

  // ============ Formatting ============
  function fmtSpeed(bps) {
    if (!bps || bps <= 0) return '...';
    if (bps < 1024) return bps.toFixed(0) + ' B/s';
    if (bps < 1048576) return (bps / 1024).toFixed(1) + ' KB/s';
    return (bps / 1048576).toFixed(1) + ' MB/s';
  }
  function fmtBytes(b) {
    if (!b || b <= 0) return '0 B';
    const u = ['B', 'KB', 'MB', 'GB'];
    let i = 0, n = b;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return n.toFixed(1) + ' ' + u[i];
  }
  function fmtEta(sec) {
    if (!sec || sec <= 0 || sec > 360000) return '--:--';
    const m = Math.floor(sec / 60), s = Math.floor(sec % 60);
    return m + 'm' + String(s).padStart(2, '0') + 's';
  }

  // ============ Task polling ============
  function startPolling(taskId) {
    const state = taskStates.get(taskId);
    if (!state || state.pollTimer) return;

    function poll() {
      apiGet('/tasks/' + taskId).then(task => {
        state.status = task.status;
        state.stage = task.stage;
        state.error = task.error_message || '';
        state.output = task.output_path || '';

        if (task.total_segments > 0) {
          state.pct = ((task.completed_segments / task.total_segments) * 100);
          if (['merging', 'remuxing'].includes(task.status)) state.pct = task.post_percent || 0;
        }
        if (task.status === 'done') state.pct = 100;

        state.speed = task.speed_bytes_per_sec || 0;
        state.size = task.downloaded_bytes || 0;
        state.segments = (task.completed_segments || 0) + '/' + (task.total_segments || 0);
        state.eta = task.eta_seconds || 0;

        updateTaskUI(taskId);

        if (['done', 'failed', 'canceled'].includes(task.status)) {
          clearInterval(state.pollTimer);
          state.pollTimer = null;
        }
      }).catch(() => {});
    }

    poll();
    state.pollTimer = setInterval(poll, 1500);
  }

  // ============ UI ============
  GM_addStyle(`
    #m3u8-banner {
      position: fixed; top: 16px; right: 16px; z-index: 999999;
      background: linear-gradient(135deg, #1e3a5f, #0f2035);
      color: #fff; border-radius: 14px; padding: 0;
      box-shadow: 0 8px 32px rgba(0,0,0,0.5);
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
      width: 400px; border: 1px solid #2563eb33;
      animation: m3u8-slide-in 0.4s ease-out;
      overflow: hidden;
    }
    @keyframes m3u8-slide-in {
      from { transform: translateX(120%); opacity: 0; }
      to   { transform: translateX(0);    opacity: 1; }
    }
    #m3u8-banner-head {
      background: #2563eb22; padding: 10px 14px;
      display: flex; justify-content: space-between; align-items: center;
      border-bottom: 1px solid #ffffff11;
    }
    #m3u8-banner-head .title { font-size: 14px; font-weight: 700; color: #60a5fa; }
    #m3u8-banner-head .count {
      background: #ef4444; color: #fff; border-radius: 50%;
      width: 22px; height: 22px; display: inline-flex;
      align-items: center; justify-content: center;
      font-size: 12px; font-weight: 700;
    }
    #m3u8-banner-body { padding: 10px 14px; max-height: 70vh; overflow-y: auto; }
    .m3u8-item {
      background: #0f172a; border-radius: 8px; padding: 10px;
      margin-bottom: 6px; border: 1px solid #1e293b;
    }
    .m3u8-item .url {
      font-size: 10px; color: #64748b; word-break: break-all;
      max-height: 28px; overflow: hidden; margin-bottom: 8px;
    }
    .m3u8-item .btns { display: flex; gap: 6px; }
    .m3u8-item .btn {
      flex: 1; padding: 7px 0; border-radius: 8px; border: none;
      cursor: pointer; font-size: 12px; font-weight: 700;
      transition: all 0.15s;
    }
    .m3u8-item .btn:active { transform: scale(0.96); }
    .m3u8-item .btn-send {
      background: linear-gradient(135deg, #22c55e, #16a34a); color: #fff;
    }
    .m3u8-item .btn-send:hover { filter: brightness(1.1); }
    .m3u8-item .btn-send.sending { background: #6b7280; cursor: wait; }
    .m3u8-item .btn-send.done { background: #059669; cursor: default; }
    .m3u8-item .btn-copy {
      background: #1e293b; color: #94a3b8; border: 1px solid #334155;
      flex: 0; padding: 7px 12px;
    }
    .m3u8-item .btn-copy:hover { background: #334155; color: #fff; }

    /* Task progress card */
    .m3u8-task {
      background: #0f172a; border-radius: 8px; padding: 10px;
      margin-bottom: 6px; border: 1px solid #1e293b;
    }
    .m3u8-task .task-title {
      font-size: 12px; font-weight: 700; color: #e2e8f0;
      margin-bottom: 6px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
    }
    .m3u8-task .task-bar-wrap {
      height: 18px; background: #1e293b; border-radius: 9px; overflow: hidden;
      position: relative; margin-bottom: 6px;
    }
    .m3u8-task .task-bar {
      height: 100%; border-radius: 9px; transition: width 0.5s;
      background: linear-gradient(90deg, #3b82f6, #60a5fa);
    }
    .m3u8-task .task-bar.done { background: linear-gradient(90deg, #16a34a, #22c55e); }
    .m3u8-task .task-bar.failed { background: linear-gradient(90deg, #ef4444, #f97316); }
    .m3u8-task .task-bar.merging { background: linear-gradient(90deg, #f59e0b, #fbbf24); }
    .m3u8-task .task-pct {
      position: absolute; inset: 0; display: flex; align-items: center;
      justify-content: center; font-size: 10px; font-weight: 700;
      color: #fff; text-shadow: 0 1px 2px rgba(0,0,0,0.5);
    }
    .m3u8-task .task-info {
      font-size: 11px; color: #94a3b8; display: flex; gap: 10px;
      flex-wrap: wrap; margin-bottom: 4px;
    }
    .m3u8-task .task-stage {
      font-size: 11px; font-weight: 600; margin-bottom: 6px;
    }
    .m3u8-task .task-stage.running { color: #60a5fa; }
    .m3u8-task .task-stage.done { color: #22c55e; }
    .m3u8-task .task-stage.failed { color: #ef4444; }
    .m3u8-task .task-stage.merging { color: #f59e0b; }
    .m3u8-task .task-actions { display: flex; gap: 6px; margin-top: 6px; }
    .m3u8-task .task-actions .btn {
      padding: 5px 10px; border-radius: 6px; border: none;
      cursor: pointer; font-size: 11px; font-weight: 600;
    }
    .m3u8-task .task-error {
      font-size: 11px; color: #f87171; background: #1a1114;
      border: 1px solid #3b1c24; border-radius: 6px; padding: 6px 8px;
      margin-top: 4px; word-break: break-all;
    }

    #m3u8-banner-close {
      background: none; border: none; color: #64748b;
      font-size: 16px; cursor: pointer; padding: 0 4px;
    }
    #m3u8-banner-close:hover { color: #fff; }
    #m3u8-toast {
      position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%);
      background: #0f172a; padding: 14px 24px;
      border-radius: 12px; font-size: 14px; font-weight: 700;
      box-shadow: 0 4px 24px rgba(0,0,0,0.6); z-index: 9999999;
      animation: m3u8-toast-in 0.3s ease-out;
    }
    #m3u8-toast.success { color: #22c55e; border: 1px solid #22c55e33; }
    #m3u8-toast.error { color: #ef4444; border: 1px solid #ef444433; }
    #m3u8-toast.info { color: #60a5fa; border: 1px solid #60a5fa33; }
    @keyframes m3u8-toast-in {
      from { transform: translate(-50%, -50%) scale(0.8); opacity: 0; }
      to   { transform: translate(-50%, -50%) scale(1);   opacity: 1; }
    }
  `);

  function showBanner() {
    if (bannerEl) { bannerEl.remove(); bannerEl = null; }

    const el = document.createElement('div');
    el.id = 'm3u8-banner';

    const items = [...found.values()].map((info, i) => `
      <div class="m3u8-item" id="m3u8-found-${i}">
        <div class="url">${info.url}</div>
        <div class="btns">
          <button class="btn btn-send" data-idx="${i}" data-url="${encodeURIComponent(info.url)}">一键下载</button>
          <button class="btn btn-copy" data-idx="${i}" data-url="${encodeURIComponent(info.url)}">复制链接</button>
        </div>
      </div>
    `).join('');

    // Build task cards for existing tasks
    const taskCards = [...taskStates.entries()].map(([id, s]) => buildTaskCardHTML(id, s)).join('');

    el.innerHTML = `
      <div id="m3u8-banner-head">
        <span class="title">m3u8 下载器</span>
        <div>
          <span class="count">${found.size}</span>
          <button id="m3u8-banner-close">✕</button>
        </div>
      </div>
      <div id="m3u8-banner-body">
        ${taskCards}
        ${items}
      </div>
    `;

    document.body.appendChild(el);
    bannerEl = el;

    el.querySelector('#m3u8-banner-close').onclick = () => { el.remove(); bannerEl = null; };
    el.querySelectorAll('.btn-send').forEach(btn => { btn.onclick = () => send(btn); });
    el.querySelectorAll('.btn-copy').forEach(btn => {
      btn.onclick = () => {
        const url = decodeURIComponent(btn.dataset.url);
        GM_setClipboard(url);
        toast('已复制 m3u8 链接', 'info');
      };
    });
  }

  function buildTaskCardHTML(taskId, state) {
    const isRunning = ['downloading', 'downloading_m3u8', 'downloading_segments', 'parsing', 'pausing', 'merging', 'remuxing'].includes(state.status);
    const isDone = state.status === 'done';
    const isFailed = state.status === 'failed';
    const isMerging = ['merging', 'remuxing'].includes(state.status);
    const pct = Math.min(100, state.pct || 0).toFixed(1);
    const barClass = isDone ? 'done' : isFailed ? 'failed' : isMerging ? 'merging' : '';
    const stageClass = isDone ? 'done' : isFailed ? 'failed' : isMerging ? 'merging' : 'running';

    const stageLabel = {
      'queued': '排队中', 'downloading_m3u8': '获取清单', 'parsing': '解析中',
      'downloading_segments': '下载分片', 'downloading': '下载中',
      'pausing': '暂停中',
      'merging': '合并中', 'remuxing': '转封装中',
      'done': '已完成', 'failed': '已失败', 'canceled': '已取消',
      'paused': '已暂停',
    }[state.status] || state.status;

    let actions = '';
    if (isDone && state.output) {
      const fileName = state.output.split(/[/\\]/).pop();
      const dir = state.output.replace(/[/\\][^/\\]+$/, '');
      actions = `
        <div class="task-actions">
          <button class="btn" style="background:#22c55e;color:#000" onclick="window._m3u8_openFile('${taskId}')">打开文件</button>
          <button class="btn" style="background:#3b82f6;color:#fff" onclick="window._m3u8_openDir('${taskId}')">打开目录</button>
          <button class="btn" style="background:#374151;color:#e1e4e8" onclick="window._m3u8_copyPath('${taskId}')">复制路径</button>
        </div>
        <div style="font-size:10px;color:#4b5563;margin-top:4px;word-break:break-all">${fileName}</div>
      `;
    } else if (isFailed && state.error) {
      const shortErr = state.error.length > 60 ? state.error.slice(0, 60) + '...' : state.error;
      actions = `<div class="task-error">${shortErr}</div>
        <div class="task-actions">
          <button class="btn" style="background:#f59e0b;color:#000" onclick="window._m3u8_retry('${taskId}')">重试</button>
        </div>`;
    }

    return `
      <div class="m3u8-task" id="m3u8-task-${taskId}">
        <div class="task-title">${state.title || taskId}</div>
        <div class="task-stage ${stageClass}">${stageLabel}${isDone && state.size ? ' · ' + fmtBytes(state.size) : ''}</div>
        <div class="task-bar-wrap">
          <div class="task-bar ${barClass}" style="width:${pct}%"></div>
          <span class="task-pct">${pct}%</span>
        </div>
        ${isRunning ? `<div class="task-info">
          <span>${state.segments || ''}</span>
          <span style="color:#22c55e">${fmtSpeed(state.speed)}</span>
          <span>ETA ${fmtEta(state.eta)}</span>
          <span>${fmtBytes(state.size)}</span>
        </div>` : ''}
        ${actions}
      </div>
    `;
  }

  function updateTaskUI(taskId) {
    const state = taskStates.get(taskId);
    if (!state || !bannerEl) return;

    const container = bannerEl.querySelector('#m3u8-banner-body');
    if (!container) return;

    let taskEl = container.querySelector('#m3u8-task-' + taskId);
    const html = buildTaskCardHTML(taskId, state);

    if (taskEl) {
      const temp = document.createElement('div');
      temp.innerHTML = html;
      taskEl.replaceWith(temp.firstElementChild);
    } else {
      // Insert at top of body (before found items)
      const firstItem = container.querySelector('.m3u8-item');
      const temp = document.createElement('div');
      temp.innerHTML = html;
      if (firstItem) {
        container.insertBefore(temp.firstElementChild, firstItem);
      } else {
        container.appendChild(temp.firstElementChild);
      }
    }
  }

  // ============ Actions ============
  function send(btn) {
    const url = decodeURIComponent(btn.dataset.url);
    const idx = parseInt(btn.dataset.idx);
    btn.textContent = '发送中...';
    btn.classList.add('sending');
    btn.disabled = true;

    const data = {
      url: url,
      referer: location.href,
      origin: location.origin,
      user_agent: navigator.userAgent,
      cookie: document.cookie,
      title: document.title,
    };

    apiPost('/tasks', data).then(task => {
      btn.textContent = '已发送 ✓';
      btn.classList.remove('sending');
      btn.classList.add('done');

      // Create task state and start polling
      taskStates.set(task.id, {
        status: task.status,
        title: task.title || task.filename || task.id,
        stage: task.stage,
        pct: 0, speed: 0, size: 0, segments: '', eta: 0,
        output: task.output_path || '',
        error: '',
        pollTimer: null,
      });

      // Refresh banner to show task card
      if (bannerEl) { showBanner(); }

      startPolling(task.id);
      toast('已发送到下载器，正在下载...', 'success');
    }).catch(() => {
      btn.textContent = '重试';
      btn.classList.remove('sending');
      btn.disabled = false;
      toast('无法连接下载器，确认后端已启动', 'error');
    });
  }

  // Global action handlers (called from inline onclick)
  window._m3u8_openFile = function (taskId) {
    const state = taskStates.get(taskId);
    if (state && state.output) {
      apiPost('/open-explorer', { path: state.output });
    }
  };

  window._m3u8_openDir = function (taskId) {
    const state = taskStates.get(taskId);
    if (state && state.output) {
      const dir = state.output.replace(/[/\\][^/\\]+$/, '');
      apiPost('/open-explorer', { path: dir });
    }
  };

  window._m3u8_copyPath = function (taskId) {
    const state = taskStates.get(taskId);
    if (state && state.output) {
      GM_setClipboard(state.output);
      toast('已复制路径', 'info');
    }
  };

  window._m3u8_retry = function (taskId) {
    apiPost('/tasks/' + taskId + '/retry', {}).then(() => {
      const state = taskStates.get(taskId);
      if (state) {
        state.status = 'downloading_segments';
        state.error = '';
        state.pct = 0;
      }
      startPolling(taskId);
      if (bannerEl) showBanner();
      toast('正在重试...', 'info');
    });
  };

  function toast(msg, type) {
    const t = document.createElement('div');
    t.id = 'm3u8-toast';
    t.className = type || 'info';
    t.textContent = msg;
    document.body.appendChild(t);
    setTimeout(() => t.remove(), 3000);
  }
})();
