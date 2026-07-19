// ==UserScript==
// @name         HLS Downloader - m3u8 一键下载
// @namespace    hls-downloader
// @version      4.3.0
// @description  自动发现网页中的 m3u8，折叠悬浮、一键下载并管理下载任务
// @compatible   Tampermonkey
// @compatible   ScriptCat
// @match        https://*/*
// @match        http://*/*
// @exclude      http://127.0.0.1/*
// @exclude      http://localhost/*
// @grant        GM_xmlhttpRequest
// @grant        GM_addStyle
// @grant        GM_setClipboard
// @grant        GM_getValue
// @grant        GM_setValue
// @connect      127.0.0.1
// @run-at       document-idle
// @noframes
// ==/UserScript==

(function () {
  'use strict';

  // These values are replaced with the current desktop settings when exported.
  const API_BASE = 'http://127.0.0.1:8765/api';
  const TOKEN = '55555'; // Must match config.json token
  const SCRIPT_VERSION = '4.3.0';

  const found = new Map();
  const taskStates = new Map();
  let panelEl = null;
  let panelCollapsed = GM_getValue('hls_panel_collapsed', true);
  let panelSide = GM_getValue('hls_panel_side', 'right') === 'left' ? 'left' : 'right';
  let activeTab = GM_getValue('hls_panel_tab', 'resources') === 'tasks' ? 'tasks' : 'resources';
  let serviceOnline = false;
  let serviceVersion = '';

  function authHeaders() {
    return { 'Content-Type': 'application/json', 'X-Token': TOKEN };
  }

  function escapeHTML(value) {
    return String(value ?? '')
      .replaceAll('&', '&amp;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;')
      .replaceAll('"', '&quot;')
      .replaceAll("'", '&#39;');
  }

  function apiRequest(method, path, data) {
    return new Promise((resolve, reject) => {
      GM_xmlhttpRequest({
        method,
        url: API_BASE + path,
        headers: authHeaders(),
        data: data === undefined ? undefined : JSON.stringify(data),
        timeout: 10000,
        onload: response => {
          let body = {};
          try {
            body = response.responseText ? JSON.parse(response.responseText) : {};
          } catch {
            reject(new Error(`本地服务返回了无效数据（HTTP ${response.status}）`));
            return;
          }
          if (response.status < 200 || response.status >= 300) {
            reject(new Error(body.detail || `HTTP ${response.status}`));
            return;
          }
          resolve(body);
        },
        ontimeout: () => reject(new Error('连接本地下载器超时')),
        onerror: () => reject(new Error('无法连接本地下载器')),
      });
    });
  }

  const apiGet = path => apiRequest('GET', path);
  const apiPost = (path, data = {}) => apiRequest('POST', path, data);

  function addM3U8(rawUrl, source = 'scan') {
    if (!rawUrl || !String(rawUrl).toLowerCase().includes('.m3u8')) return false;
    let url;
    try {
      url = new URL(String(rawUrl), location.href).href;
    } catch {
      return false;
    }
    if (found.has(url)) return false;
    found.set(url, { url, source, time: Date.now(), sent: false });
    renderPanel();
    return true;
  }

  function scan() {
    try {
      performance.getEntriesByType('resource').forEach(entry => addM3U8(entry.name, '网络'));
      document.querySelectorAll('video[src], audio[src], source[src]').forEach(element => {
        addM3U8(element.currentSrc || element.src, '媒体');
      });
    } catch {}
  }

  // Hooks catch resources created after the initial page load.
  const originalFetch = window.fetch;
  if (originalFetch) {
    window.fetch = function (...args) {
      const input = typeof args[0] === 'string' ? args[0] : args[0]?.url;
      addM3U8(input, 'Fetch');
      return originalFetch.apply(this, args);
    };
  }

  const originalOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    addM3U8(typeof url === 'string' ? url : String(url), 'XHR');
    return originalOpen.call(this, method, url, ...rest);
  };

  try {
    const mediaSrc = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'src');
    if (mediaSrc?.set && mediaSrc?.get) {
      Object.defineProperty(HTMLMediaElement.prototype, 'src', {
        configurable: mediaSrc.configurable,
        enumerable: mediaSrc.enumerable,
        set(value) {
          addM3U8(value, '媒体');
          return mediaSrc.set.call(this, value);
        },
        get() {
          return mediaSrc.get.call(this);
        },
      });
    }
  } catch {}

  try {
    const observer = new PerformanceObserver(list => {
      list.getEntries().forEach(entry => addM3U8(entry.name, '网络'));
    });
    observer.observe({ entryTypes: ['resource'] });
  } catch {}

  const mediaObserver = new MutationObserver(records => {
    for (const record of records) {
      if (record.type === 'attributes') {
        addM3U8(record.target.currentSrc || record.target.src, '媒体');
      }
      record.addedNodes.forEach(node => {
        if (!(node instanceof Element)) return;
        if (node.matches?.('video[src], audio[src], source[src]')) addM3U8(node.currentSrc || node.src, '媒体');
        node.querySelectorAll?.('video[src], audio[src], source[src]').forEach(element => {
          addM3U8(element.currentSrc || element.src, '媒体');
        });
      });
    }
  });
  mediaObserver.observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] });

  function pingDownloader() {
    apiPost('/userscript/ping', {
      version: SCRIPT_VERSION,
      page_url: location.href,
    }).then(() => {
      serviceOnline = true;
      renderPanel();
    }).catch(() => {
      serviceOnline = false;
      renderPanel();
    });
  }

  function checkService(showResult = false) {
    return apiGet('/health').then(result => {
      serviceOnline = true;
      serviceVersion = result.version || '';
      renderPanel();
      if (showResult) toast(`下载器已连接${serviceVersion ? ` · v${serviceVersion}` : ''}`, 'success');
    }).catch(error => {
      serviceOnline = false;
      renderPanel();
      if (showResult) toast(error.message, 'error');
    });
  }

  function fmtSpeed(bytesPerSecond) {
    if (!bytesPerSecond || bytesPerSecond <= 0) return '--';
    if (bytesPerSecond < 1024) return bytesPerSecond.toFixed(0) + ' B/s';
    if (bytesPerSecond < 1048576) return (bytesPerSecond / 1024).toFixed(1) + ' KB/s';
    return (bytesPerSecond / 1048576).toFixed(1) + ' MB/s';
  }

  function fmtBytes(bytes) {
    if (!bytes || bytes <= 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB'];
    let index = 0;
    let value = bytes;
    while (value >= 1024 && index < units.length - 1) {
      value /= 1024;
      index += 1;
    }
    return value.toFixed(1) + ' ' + units[index];
  }

  function fmtEta(seconds) {
    if (!seconds || seconds <= 0 || seconds > 360000) return '--:--';
    const minutes = Math.floor(seconds / 60);
    const remainder = Math.floor(seconds % 60);
    return minutes + 'm' + String(remainder).padStart(2, '0') + 's';
  }

  const statusLabels = {
    queued: '排队中',
    downloading: '准备下载',
    downloading_m3u8: '获取清单',
    parsing: '解析清单',
    downloading_segments: '下载分片',
    pausing: '正在暂停',
    paused: '已暂停',
    merging: '合并视频',
    remuxing: '转封装',
    done: '已完成',
    failed: '下载失败',
    canceled: '已取消',
    unsupported: '不支持',
  };

  function startPolling(taskId) {
    const state = taskStates.get(taskId);
    if (!state || state.pollTimer) return;

    const poll = () => {
      apiGet('/tasks/' + encodeURIComponent(taskId)).then(task => {
        Object.assign(state, {
          status: task.status,
          stage: task.stage,
          error: task.error_message || '',
          errorCode: task.error_code || '',
          output: task.output_path || '',
          speed: task.speed_bytes_per_sec || 0,
          size: task.downloaded_bytes || 0,
          segments: (task.completed_segments || 0) + '/' + (task.total_segments || 0),
          eta: task.eta_seconds || 0,
          availableActions: task.available_actions || [],
          queuePosition: task.queue_position || 0,
        });
        if (task.total_segments > 0) {
          state.pct = (task.completed_segments / task.total_segments) * 100;
        }
        if (['merging', 'remuxing'].includes(task.status)) state.pct = task.post_percent || 0;
        if (task.status === 'done') state.pct = 100;
        renderPanel();

        if (['done', 'failed', 'canceled', 'unsupported'].includes(task.status)) {
          clearInterval(state.pollTimer);
          state.pollTimer = null;
        }
      }).catch(error => {
        serviceOnline = false;
        state.connectionError = error.message;
        renderPanel();
      });
    };

    poll();
    state.pollTimer = setInterval(poll, 1500);
  }

  GM_addStyle(`
    #hls-helper-panel { all: initial; }
    #hls-helper-panel, #hls-helper-panel * { box-sizing: border-box; letter-spacing: 0; }
    #hls-helper-panel * { font-family: inherit; }
    #hls-helper-panel header, #hls-helper-panel nav, #hls-helper-panel main,
    #hls-helper-panel footer, #hls-helper-panel article { margin: 0; padding: 0; width: auto; min-width: 0; min-height: 0; }
    #hls-helper-panel {
      --hls-bg: #171a1f; --hls-surface: #20242a; --hls-raised: #292e35;
      --hls-border: #3a414a; --hls-text: #edf1f5; --hls-muted: #98a2ad;
      --hls-accent: #2b8ac6; --hls-green: #39a875; --hls-amber: #d59a3a; --hls-red: #dc6262;
      position: fixed; top: 18px; z-index: 2147483646; width: 368px;
      color: var(--hls-text); background: var(--hls-bg); border: 1px solid var(--hls-border);
      border-radius: 8px; box-shadow: 0 14px 44px rgba(0,0,0,.42);
      font: 13px/1.4 -apple-system, BlinkMacSystemFont, "Segoe UI", "Microsoft YaHei", sans-serif;
      overflow: hidden; transition: width .18s ease, height .18s ease, opacity .18s ease;
    }
    #hls-helper-panel.hls-side-right { right: 18px; }
    #hls-helper-panel.hls-side-left { left: 18px; }
    #hls-helper-panel.hls-collapsed { width: 48px; height: 48px; border-radius: 8px; }
    #hls-helper-panel button, #hls-helper-panel input { font: inherit; letter-spacing: 0; }
    #hls-helper-panel button { color: inherit; }
    #hls-helper-panel button:focus-visible, #hls-helper-panel input:focus-visible { outline: 2px solid var(--hls-accent); outline-offset: 1px; }
    .hls-head { display: flex; align-items: center; min-height: 48px; padding: 0 8px 0 10px; background: var(--hls-surface); border-bottom: 1px solid var(--hls-border); }
    .hls-brand { display: flex; align-items: center; gap: 8px; min-width: 0; padding: 0; border: 0; background: transparent; cursor: default; }
    .hls-logo { display: grid; place-items: center; width: 28px; height: 28px; flex: 0 0 auto; border-radius: 6px; background: var(--hls-accent); color: #fff; font-size: 17px; font-weight: 800; }
    .hls-title { min-width: 0; text-align: left; }
    .hls-title strong, .hls-title small { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .hls-title strong { font-size: 13px; }
    .hls-title small { margin-top: 1px; color: var(--hls-muted); font-size: 10px; }
    .hls-head-actions { display: flex; align-items: center; gap: 3px; margin-left: auto; }
    .hls-icon-btn { display: grid; place-items: center; width: 28px; height: 28px; padding: 0; border: 0; border-radius: 5px; background: transparent; color: var(--hls-muted)!important; cursor: pointer; }
    .hls-icon-btn:hover { background: var(--hls-raised); color: var(--hls-text)!important; }
    .hls-service-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--hls-red); }
    .hls-service-dot.online { background: var(--hls-green); }
    .hls-compact-badge { display: none; position: absolute; top: -5px; right: -5px; min-width: 18px; height: 18px; padding: 0 4px; border: 2px solid var(--hls-bg); border-radius: 9px; background: var(--hls-red); color: #fff; font-size: 10px; line-height: 14px; text-align: center; }
    .hls-collapsed .hls-head { width: 48px; height: 48px; padding: 9px; border: 0; cursor: pointer; }
    .hls-collapsed .hls-brand { cursor: pointer; }
    .hls-collapsed .hls-logo { width: 30px; height: 30px; }
    .hls-collapsed .hls-title, .hls-collapsed .hls-head-actions, .hls-collapsed .hls-shell { display: none; }
    .hls-collapsed .hls-compact-badge { display: block; }
    .hls-collapsed.hls-side-left .hls-compact-badge { right: auto; left: -5px; }
    .hls-shell, .hls-body, .hls-row { display: block; }
    .hls-tabs { display: grid; grid-template-columns: 1fr 1fr; padding: 6px 8px 0!important; background: var(--hls-surface); }
    .hls-tab { height: 32px; border: 0; border-bottom: 2px solid transparent; background: transparent; color: var(--hls-muted)!important; cursor: pointer; }
    .hls-tab.active { border-bottom-color: var(--hls-accent); color: var(--hls-text)!important; font-weight: 600; }
    .hls-toolbar { display: flex; align-items: center; gap: 5px; min-height: 40px; padding: 6px 8px; border-bottom: 1px solid var(--hls-border); background: var(--hls-surface); }
    .hls-btn { display: inline-flex; align-items: center; justify-content: center; height: 29px; padding: 0 9px; border: 1px solid var(--hls-border); border-radius: 5px; background: var(--hls-raised); color: var(--hls-text)!important; font-size: 11px!important; cursor: pointer; white-space: nowrap; }
    .hls-btn:hover:not(:disabled) { border-color: #59636f; }
    .hls-btn:disabled { opacity: .42; cursor: default; }
    .hls-btn.primary { border-color: var(--hls-accent); background: var(--hls-accent); color: #fff!important; }
    .hls-btn.success { border-color: var(--hls-green); background: var(--hls-green); color: #fff!important; }
    .hls-btn.danger { color: var(--hls-red)!important; }
    .hls-toolbar .hls-spacer { flex: 1; }
    .hls-body { width: auto!important; max-height: min(58vh, 520px); overflow: auto; margin: 0!important; padding: 8px!important; scrollbar-width: thin; scrollbar-color: #505863 transparent; }
    .hls-manual { display: grid; grid-template-columns: minmax(0,1fr) auto; gap: 6px; margin-bottom: 8px; }
    .hls-manual input { display: block; width: 100%; min-width: 0; height: 31px; margin: 0; padding: 0 8px; border: 1px solid var(--hls-border); border-radius: 5px; background: #111419; color: var(--hls-text); }
    .hls-list { display: grid; gap: 6px; }
    .hls-row { padding: 9px; border: 1px solid var(--hls-border); border-radius: 6px; background: var(--hls-surface); }
    .hls-row-head { display: flex; align-items: center; gap: 7px; min-width: 0; }
    .hls-host, .hls-task-title { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; font-weight: 600; }
    .hls-source { padding: 2px 5px; border: 1px solid var(--hls-border); border-radius: 4px; color: var(--hls-muted); font-size: 9px; }
    .hls-url, .hls-output { margin-top: 4px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--hls-muted); font: 10px/1.4 Consolas, monospace; }
    .hls-row-actions { display: flex; flex-wrap: wrap; gap: 5px; margin-top: 8px; }
    .hls-status { color: var(--hls-accent); font-size: 10px; font-weight: 600; }
    .hls-status.done { color: var(--hls-green); }
    .hls-status.failed { color: var(--hls-red); }
    .hls-status.paused, .hls-status.merging { color: var(--hls-amber); }
    .hls-progress { display: grid; grid-template-columns: minmax(0,1fr) 42px; align-items: center; gap: 7px; margin-top: 7px; }
    .hls-progress-track { height: 6px; overflow: hidden; border-radius: 3px; background: var(--hls-raised); }
    .hls-progress-bar { display: block; height: 100%; background: var(--hls-accent); transition: width .35s ease; }
    .hls-progress-bar.done { background: var(--hls-green); }
    .hls-progress-bar.failed { background: var(--hls-red); }
    .hls-progress-bar.merging { background: var(--hls-amber); }
    .hls-progress-value { color: var(--hls-muted); font-size: 10px; text-align: right; }
    .hls-task-meta { display: flex; gap: 10px; margin-top: 6px; color: var(--hls-muted); font-size: 10px; }
    .hls-error { margin-top: 7px; padding: 6px 7px; border-left: 2px solid var(--hls-red); background: #24191b; color: #ef9999; font-size: 10px; word-break: break-word; }
    .hls-empty { display: grid; place-items: center; min-height: 112px; padding: 18px; color: var(--hls-muted); text-align: center; }
    .hls-empty strong { display: block; margin-bottom: 4px; color: var(--hls-text); font-size: 12px; }
    .hls-footer { display: flex; align-items: center; gap: 7px; min-height: 37px; padding: 5px 8px; border-top: 1px solid var(--hls-border); background: var(--hls-surface); color: var(--hls-muted); font-size: 10px; }
    .hls-footer span { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    #hls-helper-toast { position: fixed; left: 50%; bottom: 38px; z-index: 2147483647; transform: translateX(-50%); max-width: min(420px, calc(100vw - 32px)); padding: 9px 13px; border: 1px solid var(--hls-toast-color, #59636f); border-radius: 6px; background: #171a1f; color: #edf1f5; box-shadow: 0 8px 28px rgba(0,0,0,.4); font: 12px/1.4 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    #hls-helper-toast.success { --hls-toast-color: #39a875; }
    #hls-helper-toast.error { --hls-toast-color: #dc6262; }
    #hls-helper-toast.info { --hls-toast-color: #2b8ac6; }
    @media (max-width: 520px) {
      #hls-helper-panel { top: 10px; width: calc(100vw - 20px); }
      #hls-helper-panel.hls-side-right { right: 10px; }
      #hls-helper-panel.hls-side-left { left: 10px; }
      #hls-helper-panel.hls-collapsed { width: 46px; height: 46px; }
      .hls-body { max-height: 52vh; }
    }
  `);

  function resourceHTML(info) {
    let host = info.url;
    let path = info.url;
    try {
      const parsed = new URL(info.url);
      host = parsed.host;
      path = parsed.pathname + parsed.search;
    } catch {}
    return `
      <article class="hls-row">
        <div class="hls-row-head">
          <span class="hls-host" title="${escapeHTML(info.url)}">${escapeHTML(host)}</span>
          <span class="hls-source">${escapeHTML(info.source)}</span>
        </div>
        <div class="hls-url" title="${escapeHTML(info.url)}">${escapeHTML(path)}</div>
        <div class="hls-row-actions">
          <button class="hls-btn primary" data-resource-action="download" data-url="${encodeURIComponent(info.url)}" ${info.sent ? 'disabled' : ''}>${info.sent ? '已发送' : '下载'}</button>
          <button class="hls-btn" data-resource-action="copy" data-url="${encodeURIComponent(info.url)}">复制链接</button>
          <button class="hls-btn danger" data-resource-action="remove" data-url="${encodeURIComponent(info.url)}">移除</button>
        </div>
      </article>
    `;
  }

  function taskHTML(taskId, state) {
    const isDone = state.status === 'done';
    const isFailed = ['failed', 'unsupported'].includes(state.status);
    const isMerging = ['merging', 'remuxing'].includes(state.status);
    const isPaused = state.status === 'paused';
    const percent = Math.max(0, Math.min(100, state.pct || 0));
    const tone = isDone ? 'done' : isFailed ? 'failed' : isMerging ? 'merging' : isPaused ? 'paused' : '';
    const actions = state.availableActions?.length ? state.availableActions : (() => {
      if (state.status === 'downloading_segments') return ['pause', 'cancel'];
      if (state.status === 'paused') return ['resume', 'cancel'];
      if (['failed', 'canceled', 'unsupported'].includes(state.status)) return ['retry'];
      if (state.status === 'done' && state.output) return ['launch', 'open'];
      if (!['done', 'failed', 'canceled', 'unsupported'].includes(state.status)) return ['cancel'];
      return [];
    })();
    const queueText = state.status === 'queued' && state.queuePosition ? ` · 第 ${state.queuePosition} 位` : '';
    const actionButton = (action, label, className = '') => actions.includes(action)
      ? `<button class="hls-btn ${className}" data-task-action="${action}" data-task-id="${escapeHTML(taskId)}">${label}</button>`
      : '';

    return `
      <article class="hls-row">
        <div class="hls-row-head">
          <span class="hls-task-title" title="${escapeHTML(state.title || taskId)}">${escapeHTML(state.title || taskId)}</span>
          <span class="hls-status ${tone}">${escapeHTML(statusLabels[state.status] || state.status || '等待')}${queueText}</span>
        </div>
        <div class="hls-progress">
          <div class="hls-progress-track"><i class="hls-progress-bar ${tone}" style="width:${percent.toFixed(1)}%"></i></div>
          <span class="hls-progress-value">${percent.toFixed(1)}%</span>
        </div>
        <div class="hls-task-meta">
          <span>${escapeHTML(state.segments || '0/0')}</span>
          <span>${escapeHTML(fmtSpeed(state.speed))}</span>
          <span>${escapeHTML(fmtEta(state.eta))}</span>
          <span>${escapeHTML(fmtBytes(state.size))}</span>
        </div>
        ${state.error ? `<div class="hls-error"><b>${escapeHTML(state.errorCode || '下载失败')}</b> · ${escapeHTML(state.error)}</div>` : ''}
        ${state.output ? `<div class="hls-output" title="${escapeHTML(state.output)}">${escapeHTML(state.output)}</div>` : ''}
        <div class="hls-row-actions">
          ${actionButton('pause', '暂停')}
          ${actionButton('resume', '继续', 'primary')}
          ${actionButton('cancel', '取消', 'danger')}
          ${actionButton('retry', '重试', 'primary')}
          ${actionButton('launch', '播放文件', 'success')}
          ${actionButton('open', '所在位置')}
          ${state.output ? `<button class="hls-btn" data-task-action="copy-path" data-task-id="${escapeHTML(taskId)}">复制路径</button>` : ''}
        </div>
      </article>
    `;
  }

  function renderPanel() {
    if (!document.body) return;
    const draft = panelEl?.querySelector('#hls-manual-url')?.value || '';
    const oldBody = panelEl?.querySelector('.hls-body');
    const oldScroll = oldBody?.scrollTop || 0;
    if (!panelEl) {
      panelEl = document.createElement('aside');
      panelEl.id = 'hls-helper-panel';
      panelEl.setAttribute('aria-label', 'HLS Downloader 嗅探助手');
      document.body.appendChild(panelEl);
    }

    panelEl.className = `hls-side-${panelSide}${panelCollapsed ? ' hls-collapsed' : ''}`;
    const resources = [...found.values()];
    const tasks = [...taskStates.entries()];
    const terminalCount = tasks.filter(([, state]) => ['done', 'failed', 'canceled', 'unsupported'].includes(state.status)).length;
    const resourceBody = `
      <div class="hls-manual">
        <input id="hls-manual-url" value="${escapeHTML(draft)}" placeholder="粘贴 m3u8 链接" aria-label="m3u8 链接">
        <button class="hls-btn" id="hls-add-url">添加</button>
      </div>
      ${resources.length
        ? `<div class="hls-list">${resources.map(resourceHTML).join('')}</div>`
        : '<div class="hls-empty"><div><strong>还没有发现播放清单</strong>播放视频后点击“重新扫描”，也可以手动粘贴链接</div></div>'}
    `;
    const taskBody = tasks.length
      ? `<div class="hls-list">${tasks.map(([id, state]) => taskHTML(id, state)).join('')}</div>`
      : '<div class="hls-empty"><div><strong>暂无脚本下载任务</strong>从“资源”中选择一个链接开始下载</div></div>';

    panelEl.innerHTML = `
      <header class="hls-head">
        <button class="hls-brand" id="hls-expand" title="${panelCollapsed ? '展开下载器' : 'HLS Downloader'}">
          <span class="hls-logo">↓</span>
          <span class="hls-title"><strong>HLS Downloader</strong><small>${escapeHTML(location.host)}</small></span>
          <span class="hls-compact-badge">${found.size}</span>
        </button>
        <div class="hls-head-actions">
          <i class="hls-service-dot ${serviceOnline ? 'online' : ''}" title="${serviceOnline ? '本地下载器已连接' : '本地下载器未连接'}"></i>
          <button class="hls-icon-btn" id="hls-switch-side" title="移到${panelSide === 'right' ? '左' : '右'}侧">↔</button>
          <button class="hls-icon-btn" id="hls-collapse" title="折叠">−</button>
        </div>
      </header>
      <div class="hls-shell">
        <nav class="hls-tabs">
          <button class="hls-tab ${activeTab === 'resources' ? 'active' : ''}" data-tab="resources">资源 ${found.size}</button>
          <button class="hls-tab ${activeTab === 'tasks' ? 'active' : ''}" data-tab="tasks">任务 ${taskStates.size}</button>
        </nav>
        <div class="hls-toolbar">
          ${activeTab === 'resources' ? `
            <button class="hls-btn primary" id="hls-download-all" ${resources.every(item => item.sent) ? 'disabled' : ''}>全部下载</button>
            <button class="hls-btn" id="hls-copy-all" ${!resources.length ? 'disabled' : ''}>复制全部</button>
            <span class="hls-spacer"></span>
            <button class="hls-btn" id="hls-rescan">重新扫描</button>
            <button class="hls-btn danger" id="hls-clear-resources" ${!resources.length ? 'disabled' : ''}>清空</button>
          ` : `
            <button class="hls-btn" id="hls-open-app">打开桌面端</button>
            <span class="hls-spacer"></span>
            <button class="hls-btn danger" id="hls-clear-finished" ${!terminalCount ? 'disabled' : ''}>隐藏已结束</button>
          `}
        </div>
        <main class="hls-body">${activeTab === 'resources' ? resourceBody : taskBody}</main>
        <footer class="hls-footer">
          <i class="hls-service-dot ${serviceOnline ? 'online' : ''}"></i>
          <span>${serviceOnline ? `本地服务正常${serviceVersion ? ` · v${escapeHTML(serviceVersion)}` : ''}` : '未连接本地下载器'}</span>
          <button class="hls-btn" id="hls-check-service">检测连接</button>
        </footer>
      </div>
    `;

    bindPanelEvents();
    const newBody = panelEl.querySelector('.hls-body');
    if (newBody) newBody.scrollTop = oldScroll;
  }

  function setCollapsed(value) {
    panelCollapsed = Boolean(value);
    GM_setValue('hls_panel_collapsed', panelCollapsed);
    renderPanel();
  }

  function setActiveTab(tab) {
    activeTab = tab;
    GM_setValue('hls_panel_tab', activeTab);
    renderPanel();
  }

  function bindPanelEvents() {
    if (!panelEl) return;
    panelEl.querySelector('#hls-expand')?.addEventListener('click', () => {
      if (panelCollapsed) setCollapsed(false);
    });
    panelEl.querySelector('#hls-collapse')?.addEventListener('click', () => setCollapsed(true));
    panelEl.querySelector('#hls-switch-side')?.addEventListener('click', () => {
      panelSide = panelSide === 'right' ? 'left' : 'right';
      GM_setValue('hls_panel_side', panelSide);
      renderPanel();
    });
    panelEl.querySelectorAll('[data-tab]').forEach(button => {
      button.addEventListener('click', () => setActiveTab(button.dataset.tab));
    });
    panelEl.querySelector('#hls-check-service')?.addEventListener('click', () => checkService(true));
    panelEl.querySelector('#hls-open-app')?.addEventListener('click', () => {
      apiPost('/app/activate').then(() => toast('已打开桌面下载器', 'success')).catch(error => toast(error.message, 'error'));
    });
    panelEl.querySelector('#hls-rescan')?.addEventListener('click', () => {
      const before = found.size;
      scan();
      toast(found.size > before ? `新发现 ${found.size - before} 个资源` : '没有发现新资源', 'info');
    });
    panelEl.querySelector('#hls-clear-resources')?.addEventListener('click', () => {
      found.clear();
      renderPanel();
    });
    panelEl.querySelector('#hls-copy-all')?.addEventListener('click', () => {
      GM_setClipboard([...found.keys()].join('\n'));
      toast(`已复制 ${found.size} 个链接`, 'success');
    });
    panelEl.querySelector('#hls-download-all')?.addEventListener('click', () => sendAll());
    panelEl.querySelector('#hls-clear-finished')?.addEventListener('click', () => {
      for (const [taskId, state] of taskStates) {
        if (['done', 'failed', 'canceled', 'unsupported'].includes(state.status)) taskStates.delete(taskId);
      }
      renderPanel();
    });

    const manualInput = panelEl.querySelector('#hls-manual-url');
    const addManual = () => {
      const value = manualInput?.value.trim();
      if (!value) return;
      if (addM3U8(value, '手动')) {
        toast('已添加链接', 'success');
      } else {
        toast('请输入有效的 m3u8 链接', 'error');
      }
    };
    panelEl.querySelector('#hls-add-url')?.addEventListener('click', addManual);
    manualInput?.addEventListener('keydown', event => {
      if (event.key === 'Enter') addManual();
    });

    panelEl.querySelectorAll('[data-resource-action]').forEach(button => {
      button.addEventListener('click', () => {
        const url = decodeURIComponent(button.dataset.url || '');
        if (button.dataset.resourceAction === 'download') sendResource(url, button);
        if (button.dataset.resourceAction === 'copy') {
          GM_setClipboard(url);
          toast('已复制链接', 'success');
        }
        if (button.dataset.resourceAction === 'remove') {
          found.delete(url);
          renderPanel();
        }
      });
    });
    panelEl.querySelectorAll('[data-task-action]').forEach(button => {
      button.addEventListener('click', () => runTaskAction(button.dataset.taskId, button.dataset.taskAction));
    });
  }

  function requestData(url) {
    return {
      url,
      referer: location.href,
      origin: location.origin,
      user_agent: navigator.userAgent,
      cookie: document.cookie,
      title: document.title,
    };
  }

  function sendResource(url, button) {
    const resource = found.get(url);
    if (!resource || resource.sent) return Promise.resolve();
    resource.sent = true;
    if (button) {
      button.textContent = '发送中';
      button.disabled = true;
    }
    return apiPost('/tasks', requestData(url)).then(task => {
      serviceOnline = true;
      taskStates.set(task.id, {
        status: task.status,
        title: task.title || task.filename || task.id,
        stage: task.stage,
        pct: 0,
        speed: 0,
        size: 0,
        segments: '0/0',
        eta: 0,
        output: task.output_path || '',
        error: '',
        errorCode: '',
        availableActions: task.available_actions || [],
        queuePosition: task.queue_position || 0,
        pollTimer: null,
      });
      activeTab = 'tasks';
      GM_setValue('hls_panel_tab', activeTab);
      setCollapsed(false);
      startPolling(task.id);
      toast('任务已发送到桌面下载器', 'success');
      return true;
    }).catch(error => {
      resource.sent = false;
      serviceOnline = false;
      renderPanel();
      toast(`发送失败：${error.message}`, 'error');
      return false;
    });
  }

  async function sendAll() {
    const pending = [...found.values()].filter(item => !item.sent);
    if (!pending.length) return;
    let success = 0;
    for (const item of pending) {
      try {
        if (await sendResource(item.url)) success += 1;
      } catch {}
    }
    toast(`已发送 ${success}/${pending.length} 个任务`, success ? 'success' : 'error');
  }

  function runTaskAction(taskId, action) {
    const state = taskStates.get(taskId);
    if (!state) return;
    if (action === 'copy-path') {
      GM_setClipboard(state.output || '');
      toast('已复制文件路径', 'success');
      return;
    }
    if (action === 'launch') {
      apiPost('/launch-file', { path: state.output }).catch(error => toast(error.message, 'error'));
      return;
    }
    if (action === 'open') {
      apiPost('/open-explorer', { path: state.output }).catch(error => toast(error.message, 'error'));
      return;
    }

    apiPost('/tasks/' + encodeURIComponent(taskId) + '/' + action).then(() => {
      state.error = '';
      if (action === 'pause') state.status = 'pausing';
      if (action === 'resume' || action === 'retry') state.status = 'queued';
      if (action === 'cancel') state.status = 'canceled';
      startPolling(taskId);
      renderPanel();
      toast('操作已提交', 'success');
    }).catch(error => toast(error.message, 'error'));
  }

  function toast(message, type = 'info') {
    document.querySelector('#hls-helper-toast')?.remove();
    const element = document.createElement('div');
    element.id = 'hls-helper-toast';
    element.className = type;
    element.textContent = message;
    document.body.appendChild(element);
    setTimeout(() => element.remove(), 2600);
  }

  renderPanel();
  scan();
  setTimeout(scan, 1200);
  setTimeout(scan, 3500);
  setInterval(scan, 5000);
  setTimeout(pingDownloader, 500);
  setInterval(pingDownloader, 60000);
  setTimeout(checkService, 900);
})();
