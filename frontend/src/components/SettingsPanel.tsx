import { useEffect, useState } from 'react'
import { ChevronDown, ChevronRight, Copy, Download, FolderOpen, RefreshCw, Trash2, X } from 'lucide-react'
import { fetchSettings, fetchUpdateInfo, installUpdate, openExplorer, saveSettings, testConnection } from '../api'
import { beginUninstall, getDesktopInfo } from '../desktop'
import { LEGACY_REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'
import type { UpdateInfo } from '../types'
import FolderPicker from './FolderPicker'

export default function SettingsPanel({ onClose }: { onClose: () => void }) {
  const [settings, setSettings] = useState<any>({})
  const [original, setOriginal] = useState<any>({})
  const [saved, setSaved] = useState(false)
  const [copied, setCopied] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [showPicker, setShowPicker] = useState(false)
  const [uninstallAvailable, setUninstallAvailable] = useState(false)
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [installingUpdate, setInstallingUpdate] = useState(false)
  const [environment, setEnvironment] = useState<any>(null)
  const [checkingEnvironment, setCheckingEnvironment] = useState(false)
  const dirty = JSON.stringify(settings) !== JSON.stringify(original)

  useEffect(() => {
    fetchSettings().then(data => { setSettings(data); setOriginal(data) }).catch(reason => setError(reason.message || '加载设置失败'))
    getDesktopInfo().then(info => setUninstallAvailable(info.installed === true))
    fetchUpdateInfo().then(setUpdateInfo).catch(() => {})
  }, [])

  const requestClose = () => {
    if (dirty && !window.confirm('设置尚未保存，确定关闭吗？')) return
    onClose()
  }

  useEffect(() => {
    const close = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      if (showPicker) setShowPicker(false)
      else requestClose()
    }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [dirty, showPicker])

  const update = (key: string, value: unknown) => setSettings((current: any) => ({ ...current, [key]: value }))
  const doSave = async () => {
    setError('')
    if (!String(settings.download_dir || '').trim()) { setError('下载保存目录不能为空'); return }
    if (settings.default_concurrency < 1 || settings.default_concurrency > 64) { setError('默认并发数必须在 1 到 64 之间'); return }
    if (settings.max_concurrent_tasks < 1 || settings.max_concurrent_tasks > 16) { setError('最大同时任务数必须在 1 到 16 之间'); return }
    if (settings.http_chunk_size_mb < 1 || settings.http_chunk_size_mb > 64) { setError('HTTP 分段大小必须在 1 到 64 MiB 之间'); return }
    if (settings.bt_upload_limit_kib < 0) { setError('BT 上传限制不能小于 0'); return }
    if (!String(settings.ffmpeg_path || '').trim()) { setError('ffmpeg 路径不能为空'); return }
    setSaving(true)
    try {
      const normalized = await saveSettings(settings)
      setSettings(normalized); setOriginal(normalized)
      localStorage.setItem('hls_token', normalized.token || '55555')
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2000)
    } catch (reason: any) {
      setError(reason.message || '保存设置失败')
    } finally { setSaving(false) }
  }
  const copyToken = async () => {
    try {
      await navigator.clipboard.writeText(settings.token || '')
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1200)
    } catch {
      setError('无法复制，请手动选择 Token')
    }
  }
  const checkEnvironment = async () => {
    setCheckingEnvironment(true); setError('')
    try { setEnvironment(await testConnection()) }
    catch (reason: any) { setError(reason.message || '环境检查失败') }
    finally { setCheckingEnvironment(false) }
  }
  const uninstall = async () => {
    setError('')
    const result = await beginUninstall()
    if (!result.ok && !result.canceled) setError(result.error || '无法启动卸载程序')
  }
  const checkUpdate = async () => {
    setCheckingUpdate(true)
    setError('')
    try {
      setUpdateInfo(await fetchUpdateInfo(true))
    } catch (reason: any) {
      setError(reason.message || '检查更新失败')
    } finally {
      setCheckingUpdate(false)
    }
  }
  const updateApp = async () => {
    if (!updateInfo?.available) return
    if (!window.confirm(`下载安装 v${updateInfo.latest_version}？下载器将在安装前自动关闭。`)) return
    setInstallingUpdate(true)
    setError('')
    try {
      await installUpdate()
    } catch (reason: any) {
      setError(reason.message || '更新失败')
      setInstallingUpdate(false)
    }
  }

  return <div className="modal-overlay" onMouseDown={requestClose}>
    <section className="modal settings-modal" onMouseDown={event => event.stopPropagation()}>
      <header><div><h2>设置{dirty ? ' *' : ''}</h2><p>下载目录、并发与请求参数</p></div><button className="icon-button" title="关闭" onClick={requestClose}><X size={18} /></button></header>
      <div className="settings-body">

      <label>Token（浏览器脚本自动使用此值）</label>
      <div className="input-action"><input value={settings.token || ''} readOnly /><button className="secondary-button" title="复制 Token" onClick={copyToken}><Copy size={15} />{copied ? '已复制' : '复制'}</button></div>

      <label>下载保存目录</label>
      <div className="input-action"><input value={settings.download_dir || ''} onChange={event => update('download_dir', event.target.value)} /><button className="secondary-button" onClick={() => setShowPicker(true)}>选择目录</button><button className="icon-button bordered" title="打开目录" onClick={() => openExplorer(settings.download_dir || '')}><FolderOpen size={17} /></button></div>
      <p className="field-note">临时分片保存在下载目录的 .tasks 子目录，完成后按设置清理。</p>

      <div className="form-row settings-number-row">
        <div><label>默认并发数</label><input type="number" min={1} max={64} value={settings.default_concurrency ?? 8} onChange={event => update('default_concurrency', Number(event.target.value))} /><p className="field-note">{REQUEST_FIELD_HELP.concurrency}</p></div>
        <div><label>最大同时任务数</label><input type="number" min={1} max={16} value={settings.max_concurrent_tasks ?? 3} onChange={event => update('max_concurrent_tasks', Number(event.target.value))} /><p className="field-note">{REQUEST_FIELD_HELP.maxTasks}</p></div>
      </div>
      <div className="settings-section-title">普通文件与浏览器接管</div>
      <div className="form-row settings-number-row">
        <div><label>HTTP 分段大小（MiB）</label><input type="number" min={1} max={64} value={settings.http_chunk_size_mb ?? 8} onChange={event => update('http_chunk_size_mb', Number(event.target.value))} /><p className="field-note">每段完成后可安全暂停；较小更灵活，较大请求更少。</p></div>
        <div><label>接管最小大小（MiB）</label><input type="number" min={0} max={10240} value={settings.browser_takeover_min_mb ?? 1} onChange={event => update('browser_takeover_min_mb', Number(event.target.value))} /><p className="field-note">小于该大小的普通文件继续交给浏览器。</p></div>
      </div>
      <label className="checkbox-label"><input type="checkbox" checked={settings.browser_takeover_enabled ?? true} onChange={event => update('browser_takeover_enabled', event.target.checked)} />启用浏览器普通文件接管</label>

      <div className="settings-section-title">BT 下载</div>
      <div className="form-row settings-number-row">
        <div><label>上传上限（KiB/s）</label><input type="number" min={0} max={1048576} value={settings.bt_upload_limit_kib ?? 1024} onChange={event => update('bt_upload_limit_kib', Number(event.target.value))} /><p className="field-note">0 表示不限速；完成后会立即停止做种。</p></div>
        <div><label>最大 Peer 连接</label><input type="number" min={10} max={1000} value={settings.bt_max_connections ?? 80} onChange={event => update('bt_max_connections', Number(event.target.value))} /><p className="field-note">限制 BT 网络连接和内存占用。</p></div>
      </div>
      <label className="checkbox-label"><input type="checkbox" checked={settings.bt_enable_dht ?? true} onChange={event => update('bt_enable_dht', event.target.checked)} />启用 DHT 节点发现</label>
      <label>默认 Referer</label><input value={settings.default_referer || ''} onChange={event => update('default_referer', event.target.value)} placeholder={LEGACY_REQUEST_EXAMPLES.referer} />
      <p className="field-note">{REQUEST_FIELD_HELP.referer}</p>

      <button className="text-button advanced-toggle" onClick={() => setShowAdvanced(value => !value)}>{showAdvanced ? <ChevronDown size={14} /> : <ChevronRight size={14} />}{showAdvanced ? '收起高级选项' : '高级选项（Origin / User-Agent / Cookie / FFmpeg）'}</button>
      {showAdvanced && <div className="advanced-settings">
        <label>默认 Origin</label><input value={settings.default_origin || ''} onChange={event => update('default_origin', event.target.value)} placeholder={LEGACY_REQUEST_EXAMPLES.origin} />
        <p className="field-note">{REQUEST_FIELD_HELP.origin}</p>
        <label>默认 User-Agent</label><input value={settings.default_user_agent || ''} onChange={event => update('default_user_agent', event.target.value)} placeholder={LEGACY_REQUEST_EXAMPLES.userAgent} />
        <p className="field-note">{REQUEST_FIELD_HELP.userAgent}</p>
        <label>默认 Cookie</label><input value={settings.default_cookie || ''} onChange={event => update('default_cookie', event.target.value)} placeholder="sessionid=abc; token=xyz" />
        <p className="field-note">{REQUEST_FIELD_HELP.cookie}</p>
        <label>ffmpeg 路径</label><div className="input-action"><input value={settings.ffmpeg_path || ''} onChange={event => update('ffmpeg_path', event.target.value)} /><button className="icon-button bordered" title="打开文件位置" onClick={() => openExplorer(settings.ffmpeg_path || '')}><FolderOpen size={17} /></button></div>
        <p className="field-note">{REQUEST_FIELD_HELP.ffmpegPath}</p>
        <label>允许的域名</label><input value={(settings.allowed_hosts || []).join(',')} onChange={event => update('allowed_hosts', event.target.value.split(',').map(value => value.trim()).filter(Boolean))} placeholder="example.com,cdn.example.com" />
        <p className="field-note">{REQUEST_FIELD_HELP.allowedHosts}</p>
        <label className="checkbox-label"><input type="checkbox" checked={settings.keep_temp_files || false} onChange={event => update('keep_temp_files', event.target.checked)} />保留临时文件（调试用）</label>
      </div>}

      {error && <div className="inline-error">{error}</div>}
      <div className="app-management">
        <div><strong>运行环境</strong><span>{environment ? `FFmpeg ${environment.ffmpeg ? '正常' : '未找到'} · 并发 ${environment.concurrency} · 同时任务 ${environment.max_tasks}` : '检查 FFmpeg、下载目录与当前并发设置'}</span></div>
        <button className="secondary-button" disabled={checkingEnvironment || dirty} title={dirty ? '请先保存设置' : '检查运行环境'} onClick={checkEnvironment}><RefreshCw size={15} />{dirty ? '保存后检查' : checkingEnvironment ? '检查中…' : '检查环境'}</button>
      </div>
      <div className="app-management">
        <div><strong>软件更新</strong><span>{updateInfo ? `当前 v${updateInfo.current_version} · ${updateInfo.available ? `可更新到 v${updateInfo.latest_version}` : '已是最新版本'}` : '尚未检查'}</span></div>
        {updateInfo?.available && updateInfo.can_auto_install
          ? <button className="primary-button" disabled={installingUpdate} onClick={updateApp}><Download size={15} />{installingUpdate ? '正在下载…' : '下载安装'}</button>
          : <button className="secondary-button" disabled={checkingUpdate} onClick={checkUpdate}><RefreshCw size={15} />{checkingUpdate ? '检查中…' : '检查更新'}</button>}
      </div>
      {uninstallAvailable && <div className="app-management">
        <div><strong>卸载程序</strong><span>删除程序、设置、任务历史和缓存</span></div>
        <button className="danger-button" onClick={uninstall}><Trash2 size={15} />卸载</button>
      </div>}
      </div>
      <footer><button className="secondary-button" onClick={requestClose}>关闭</button><button className="primary-button" disabled={!dirty || saving} onClick={doSave}>{saving ? '保存中…' : saved ? '已保存' : '保存设置'}</button></footer>
    </section>
    {showPicker && <FolderPicker initialPath={settings.download_dir || ''} onSelect={path => { update('download_dir', path); setShowPicker(false) }} onClose={() => setShowPicker(false)} />}
  </div>
}
