import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import { ChevronDown, ChevronRight, Copy, Download, FolderOpen, RefreshCw, Trash2, X } from 'lucide-react'
import { fetchSettings, fetchUpdateInfo, installUpdate, openExplorer, saveSettings, scanTvboxDevices, testConnection } from '../api'
import { beginUninstall, getDesktopInfo } from '../desktop'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'
import type { ThemePreference } from '../theme'
import type { UpdateInfo } from '../types'
import { friendlyUpdateError } from '../updateError'
import { pickFolder } from '../desktop'
import FolderPicker from './FolderPicker'
import ConfirmDialog from './ConfirmDialog'
import { Button } from './ui'

type SettingsSection = 'general' | 'network' | 'maintenance'
const SETTINGS_SECTIONS: SettingsSection[] = ['general', 'network', 'maintenance']

export default function SettingsPanel({ themePreference, onThemePreferenceChange, onClose }: {
  themePreference: ThemePreference
  onThemePreferenceChange: (theme: ThemePreference) => void
  onClose: () => void
}) {
  const [settings, setSettings] = useState<any>({})
  const [original, setOriginal] = useState<any>({})
  const [saved, setSaved] = useState(false)
  const [copied, setCopied] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [showPicker, setShowPicker] = useState(false)
  const [showTempPicker, setShowTempPicker] = useState(false)
  const [confirmAction, setConfirmAction] = useState<'close' | 'update' | null>(null)
  const [uninstallAvailable, setUninstallAvailable] = useState(false)
  const [desktopInfo, setDesktopInfo] = useState<{ shell?: string; desktop_version?: string } | null>(null)
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  const [updateError, setUpdateError] = useState('')
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [installingUpdate, setInstallingUpdate] = useState(false)
  const [environment, setEnvironment] = useState<any>(null)
  const [checkingEnvironment, setCheckingEnvironment] = useState(false)
  const [tvboxDevices, setTvboxDevices] = useState<Array<{ endpoint: string; host: string; port: number; label: string; matched: boolean }>>([])
  const [scanningTvbox, setScanningTvbox] = useState(false)
  const [tvboxScanMessage, setTvboxScanMessage] = useState('')
  const [activeSection, setActiveSection] = useState<SettingsSection>('general')
  const dialogRef = useRef<HTMLElement>(null)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const previousFocusRef = useRef<HTMLElement | null>(null)
  const dirty = JSON.stringify(settings) !== JSON.stringify(original)
  const selectedTvboxDevice = tvboxDevices.some(device => device.endpoint === settings.tvbox_endpoint)
  const tvboxSelectValue = settings.tvbox_endpoint
    ? (selectedTvboxDevice ? settings.tvbox_endpoint : '__manual__')
    : ''

  useEffect(() => {
    fetchSettings().then(data => { setSettings(data); setOriginal(data) }).catch(reason => setError(reason.message || '加载设置失败'))
    getDesktopInfo().then(info => { setUninstallAvailable(info.installed === true); setDesktopInfo(info) })
    fetchUpdateInfo().then(setUpdateInfo).catch(() => {})
  }, [])

  useEffect(() => {
    previousFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const timer = window.setTimeout(() => closeButtonRef.current?.focus(), 0)
    return () => {
      window.clearTimeout(timer)
      previousFocusRef.current?.focus()
    }
  }, [])

  const requestClose = () => {
    if (dirty) { setConfirmAction('close'); return }
    onClose()
  }

  useEffect(() => {
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Tab' && !confirmAction && !showTempPicker && !showPicker) {
        const focusable = dialogRef.current?.querySelectorAll<HTMLElement>('a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])')
        if (!focusable?.length) return
        const items = Array.from(focusable)
        const first = items[0]
        const last = items[items.length - 1]
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault()
          last.focus()
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault()
          first.focus()
        }
        return
      }
      if (event.key === 'Escape') {
        if (confirmAction) setConfirmAction(null)
        else if (showTempPicker) setShowTempPicker(false)
        else if (showPicker) setShowPicker(false)
        else requestClose()
      }
    }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [dirty, showPicker, showTempPicker, confirmAction])

  const update = (key: string, value: unknown) => setSettings((current: any) => ({ ...current, [key]: value }))
  const moveSettingsTab = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
    event.preventDefault()
    const currentIndex = SETTINGS_SECTIONS.indexOf(activeSection)
    const nextIndex = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? SETTINGS_SECTIONS.length - 1
        : (currentIndex + (event.key === 'ArrowRight' ? 1 : SETTINGS_SECTIONS.length - 1)) % SETTINGS_SECTIONS.length
    const next = SETTINGS_SECTIONS[nextIndex]
    setActiveSection(next)
    window.requestAnimationFrame(() => document.getElementById(`settings-tab-${next}`)?.focus())
  }
  const doSave = async () => {
    setError('')
    if (!String(settings.download_dir || '').trim()) { setError('下载保存目录不能为空'); return }
    if (!String(settings.temp_dir || '').trim()) { setError('缓存与过程文件目录不能为空'); return }
    if (settings.default_concurrency < 1 || settings.default_concurrency > 256) { setError('默认并发数必须在 1 到 256 之间'); return }
    if (settings.max_concurrent_tasks < 1 || settings.max_concurrent_tasks > 16) { setError('最大同时任务数必须在 1 到 16 之间'); return }
    if (settings.http_chunk_size_mb < 1 || settings.http_chunk_size_mb > 64) { setError('HTTP 分段大小必须在 1 到 64 MiB 之间'); return }
    if (settings.download_speed_limit_kib != null && settings.download_speed_limit_kib < 0) { setError('下载限速不能小于 0'); return }
    if (settings.bt_upload_limit_kib < 0) { setError('BT 上传限制不能小于 0'); return }
    if (settings.queue_auto_start_enabled && !/^([01]\d|2[0-3]):[0-5]\d$/.test(String(settings.queue_auto_start_time || ''))) { setError('定时开始时间必须为 HH:MM'); return }
    if (!String(settings.ffmpeg_path || '').trim()) { setError('ffmpeg 路径不能为空'); return }
    const tvboxEndpoint = String(settings.tvbox_endpoint || '').trim()
    if (tvboxEndpoint) {
      try {
        const parsed = new URL(tvboxEndpoint)
        if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname) throw new Error()
      } catch {
        setError('电视推送地址必须是有效的 http:// 或 https:// 地址')
        setActiveSection('network')
        return
      }
    }
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
  const scanTvbox = async () => {
    setScanningTvbox(true); setError(''); setTvboxScanMessage('正在扫描当前局域网…')
    try {
      const result = await scanTvboxDevices()
      const devices = result.devices || []
      setTvboxDevices(devices)
      if (!devices.length) {
        setTvboxScanMessage('未发现设备。请确认电脑与 TVBox 在同一局域网，或手动填写地址。')
      } else {
        setTvboxScanMessage(`发现 ${devices.length} 台设备，请选择后保存设置。`)
      }
    } catch (reason: any) {
      setTvboxScanMessage(reason.message || '扫描电视设备失败')
    } finally { setScanningTvbox(false) }
  }
  const uninstall = async () => {
    setError('')
    const result = await beginUninstall()
    if (!result.ok && !result.canceled) setError(result.error || '无法启动卸载程序')
  }
  const checkUpdate = async () => {
    setCheckingUpdate(true)
    setUpdateError('')
    try {
      setUpdateInfo(await fetchUpdateInfo(true))
    } catch (reason: any) {
      setUpdateError(friendlyUpdateError(reason, '暂时无法检查更新，请稍后重试。'))
    } finally {
      setCheckingUpdate(false)
    }
  }
  const updateApp = async (confirmed = false) => {
    if (!updateInfo?.available) return
    if (!confirmed) { setConfirmAction('update'); return }
    setInstallingUpdate(true)
    setUpdateError('')
    try {
      await installUpdate()
    } catch (reason: any) {
      setUpdateError(friendlyUpdateError(reason, '安装包下载或启动失败，请稍后重试。'))
      setInstallingUpdate(false)
    }
  }

  return <div className="modal-overlay settings-overlay" onMouseDown={requestClose}>
    <section ref={dialogRef} className="modal settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-dialog-title" onMouseDown={event => event.stopPropagation()}>
      <header className="settings-header">
        <div className="settings-title"><h2 id="settings-dialog-title">应用设置{dirty ? ' *' : ''}</h2><p>界面、下载行为与运行环境</p></div>
        <nav className="settings-tabs" role="tablist" aria-label="设置分区" onKeyDown={moveSettingsTab}>
          <button id="settings-tab-general" type="button" role="tab" aria-selected={activeSection === 'general'} aria-controls="settings-general" className={activeSection === 'general' ? 'active' : ''} onClick={() => setActiveSection('general')}>通用</button>
          <button id="settings-tab-network" type="button" role="tab" aria-selected={activeSection === 'network'} aria-controls="settings-network" className={activeSection === 'network' ? 'active' : ''} onClick={() => setActiveSection('network')}>网络与下载</button>
          <button id="settings-tab-maintenance" type="button" role="tab" aria-selected={activeSection === 'maintenance'} aria-controls="settings-maintenance" className={activeSection === 'maintenance' ? 'active' : ''} onClick={() => setActiveSection('maintenance')}>维护</button>
        </nav>
        <Button ref={closeButtonRef} variant="ghost" size="icon" className="icon-button settings-close" title="关闭" aria-label="关闭" onClick={requestClose}><X size={18} /></Button>
      </header>
      <div className="settings-body">
        {activeSection === 'general' && <div id="settings-general" role="tabpanel" aria-labelledby="settings-tab-general" className="settings-page">
          <section className="settings-group">
            <div className="settings-row settings-row-control">
              <div><strong>应用主题</strong><span>跟随系统，或固定使用浅色/深色外观</span></div>
              <select aria-label="应用主题" value={themePreference} onChange={event => onThemePreferenceChange(event.target.value as ThemePreference)}>
                <option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option>
              </select>
            </div>
            <div className="settings-row settings-row-stack">
              <div><strong>浏览器连接 Token</strong><span>浏览器扩展使用此值连接本地服务</span></div>
              <div className="input-action"><input aria-label="浏览器连接 Token" value={settings.token || ''} readOnly /><button className="secondary-button" title="复制 Token" onClick={copyToken}><Copy size={15} />{copied ? '已复制' : '复制'}</button></div>
            </div>
          </section>

          <h3 className="settings-group-label">目录与存储</h3>
          <section className="settings-group">
            <div className="settings-row settings-row-stack">
              <div><strong>下载保存目录</strong><span>最终文件保存位置；浏览器接管时可为单个任务另选目录</span></div>
              <div className="input-action"><input aria-label="下载保存目录" value={settings.download_dir || ''} onChange={event => update('download_dir', event.target.value)} /><button className="secondary-button" onClick={() => void (async () => {
                const native = await pickFolder(settings.download_dir || '')
                if (native.ok && native.path) { update('download_dir', native.path); return }
                if (native.canceled) return
                setShowPicker(true)
              })()}>选择目录</button><button className="icon-button bordered" title="打开目录" onClick={() => openExplorer(settings.download_dir || '')}><FolderOpen size={17} /></button></div>
            </div>
            <div className="settings-row settings-row-stack">
              <div><strong>缓存与过程文件目录</strong><span>分片、断点、BT 数据和任务日志保存在该目录的 .tasks 中</span></div>
              <div className="input-action"><input aria-label="缓存与过程文件目录" value={settings.temp_dir || ''} onChange={event => update('temp_dir', event.target.value)} /><button className="secondary-button" onClick={() => void (async () => {
                const native = await pickFolder(settings.temp_dir || '')
                if (native.ok && native.path) { update('temp_dir', native.path); return }
                if (native.canceled) return
                setShowTempPicker(true)
              })()}>选择目录</button><button className="icon-button bordered" title="打开目录" onClick={() => openExplorer(settings.temp_dir || '')}><FolderOpen size={17} /></button></div>
            </div>
          </section>

          <h3 className="settings-group-label">任务调度</h3>
          <section className="settings-group settings-grid-group">
            <div className="settings-field"><label htmlFor="setting-default-concurrency">默认并发数</label><input id="setting-default-concurrency" type="number" min={1} max={256} value={settings.default_concurrency ?? 12} onChange={event => update('default_concurrency', Number(event.target.value))} /><p>{REQUEST_FIELD_HELP.concurrency}</p></div>
            <div className="settings-field"><label htmlFor="setting-max-tasks">最大同时任务数</label><input id="setting-max-tasks" type="number" min={1} max={16} value={settings.max_concurrent_tasks ?? 3} onChange={event => update('max_concurrent_tasks', Number(event.target.value))} /><p>{REQUEST_FIELD_HELP.maxTasks}</p></div>
            <div className="settings-field"><label htmlFor="setting-speed-limit">全局下载限速（KiB/s）</label><input id="setting-speed-limit" type="number" min={0} max={1048576} value={settings.download_speed_limit_kib ?? 0} onChange={event => update('download_speed_limit_kib', Number(event.target.value))} /><p>{REQUEST_FIELD_HELP.speedLimit}</p></div>
            <div className="settings-field"><label htmlFor="setting-http-chunk">HTTP 分段大小（MiB）</label><input id="setting-http-chunk" type="number" min={1} max={64} value={settings.http_chunk_size_mb ?? 8} onChange={event => update('http_chunk_size_mb', Number(event.target.value))} /><p>每段完成后可安全暂停；较小更灵活，较大请求更少。</p></div>
          </section>
          <h3 className="settings-group-label">定时队列</h3>
          <section className="settings-group settings-grid-group">
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.queue_auto_start_enabled ?? false} onChange={event => update('queue_auto_start_enabled', event.target.checked)} />在指定时间自动开始新队列</label>
            <div className="settings-field"><label htmlFor="setting-queue-auto-start">自动开始时间</label><input id="setting-queue-auto-start" type="time" disabled={!settings.queue_auto_start_enabled} value={settings.queue_auto_start_time ?? '00:00'} onChange={event => update('queue_auto_start_time', event.target.value)} /><p>开启后，新任务保持排队，直到当天该时间开始。排队中可右键调整优先级（上移/下移/队首/队尾）。</p></div>
          </section>
        </div>}

        {activeSection === 'network' && <div id="settings-network" role="tabpanel" aria-labelledby="settings-tab-network" className="settings-page">
          <h3 className="settings-group-label settings-group-label-first">BT 下载</h3>
          <section className="settings-group settings-grid-group">
            <div className="settings-field"><label htmlFor="setting-bt-upload">上传上限（KiB/s）</label><input id="setting-bt-upload" type="number" min={0} max={1048576} value={settings.bt_upload_limit_kib ?? 1024} onChange={event => update('bt_upload_limit_kib', Number(event.target.value))} /><p>0 表示不限速；完成后会立即停止做种。</p></div>
            <div className="settings-field"><label htmlFor="setting-bt-peers">最大 Peer 连接</label><input id="setting-bt-peers" type="number" min={10} max={1000} value={settings.bt_max_connections ?? 80} onChange={event => update('bt_max_connections', Number(event.target.value))} /><p>限制 BT 网络连接和内存占用。</p></div>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.bt_enable_dht ?? true} onChange={event => update('bt_enable_dht', event.target.checked)} />启用 DHT 节点发现</label>
          </section>

          <h3 className="settings-group-label">电视推送（TVBox）</h3>
          <section className="settings-group settings-grid-group">
            <div className="settings-field settings-field-wide">
              <label htmlFor="setting-tvbox-endpoint">已选择的电视设备</label>
              <div className="input-action">
                <select id="setting-tvbox-endpoint" value={tvboxSelectValue} onChange={event => {
                  const value = event.target.value
                  if (value === '') update('tvbox_endpoint', '')
                  else if (value === '__manual__') {
                    if (selectedTvboxDevice) update('tvbox_endpoint', '')
                  } else update('tvbox_endpoint', value)
                }} aria-label="已选择的电视设备">
                  <option value="">不使用电视推送</option>
                  {tvboxDevices.map(device => <option key={device.endpoint} value={device.endpoint}>{device.label} · {device.host}:{device.port}</option>)}
                  <option value="__manual__">手动填写地址</option>
                </select>
                <Button variant="secondary" className="secondary-button" disabled={scanningTvbox} title="扫描同一局域网中的 TVBox" onClick={() => void scanTvbox()}><RefreshCw size={15} />{scanningTvbox ? '扫描中…' : '扫描电视'}</Button>
              </div>
              <p>桌面端扫描并记住设备；插件只交给桌面端当前视频地址，不直接访问电视。</p>
              {tvboxScanMessage && <p className="settings-inline-status" role="status" aria-live="polite">{tvboxScanMessage}</p>}
              {tvboxSelectValue === '__manual__' && <input aria-label="手动电视推送地址" value={settings.tvbox_endpoint || ''} onChange={event => update('tvbox_endpoint', event.target.value)} placeholder="http://192.168.1.100:9979 或 http://192.168.1.100:9979/action" />}
            </div>
          </section>

          <h3 className="settings-group-label">手工任务请求身份</h3>
          <section className="settings-group">
            <div className="settings-row settings-row-stack"><div><strong>默认 Referer</strong><span>{REQUEST_FIELD_HELP.referer}</span></div><input aria-label="默认 Referer" value={settings.default_referer || ''} onChange={event => update('default_referer', event.target.value)} placeholder={REQUEST_EXAMPLES.referer} /></div>
            <button className="text-button advanced-toggle" onClick={() => setShowAdvanced(value => !value)}>{showAdvanced ? <ChevronDown size={14} /> : <ChevronRight size={14} />}{showAdvanced ? '收起高级请求选项' : '展开 Origin、User-Agent、Cookie、域名与 FFmpeg'}</button>
            {showAdvanced && <div className="advanced-settings settings-advanced-grid">
              <div className="settings-field"><label htmlFor="setting-origin">默认 Origin</label><input id="setting-origin" value={settings.default_origin || ''} onChange={event => update('default_origin', event.target.value)} placeholder={REQUEST_EXAMPLES.origin} /><p>{REQUEST_FIELD_HELP.origin}</p></div>
              <div className="settings-field"><label htmlFor="setting-user-agent">默认 User-Agent</label><input id="setting-user-agent" value={settings.default_user_agent || ''} onChange={event => update('default_user_agent', event.target.value)} placeholder={REQUEST_EXAMPLES.userAgent} /><p>{REQUEST_FIELD_HELP.userAgent}</p></div>
              <div className="settings-field settings-field-wide"><label htmlFor="setting-cookie">默认 Cookie</label><input id="setting-cookie" value={settings.default_cookie || ''} onChange={event => update('default_cookie', event.target.value)} placeholder="sessionid=abc; token=xyz" /><p>{REQUEST_FIELD_HELP.cookie}</p></div>
              <div className="settings-field"><label htmlFor="setting-ffmpeg">ffmpeg 路径</label><div className="input-action"><input id="setting-ffmpeg" value={settings.ffmpeg_path || ''} onChange={event => update('ffmpeg_path', event.target.value)} /><button className="icon-button bordered" title="打开文件位置" onClick={() => openExplorer(settings.ffmpeg_path || '')}><FolderOpen size={17} /></button></div><p>{REQUEST_FIELD_HELP.ffmpegPath}</p></div>
              <div className="settings-field"><label htmlFor="setting-allowed-hosts">允许的域名</label><input id="setting-allowed-hosts" value={(settings.allowed_hosts || []).join(',')} onChange={event => update('allowed_hosts', event.target.value.split(',').map(value => value.trim()).filter(Boolean))} placeholder="example.com,cdn.example.com" /><p>{REQUEST_FIELD_HELP.allowedHosts}</p></div>
              <label className="checkbox-label settings-checkbox settings-field-wide"><input type="checkbox" checked={settings.keep_temp_files || false} onChange={event => update('keep_temp_files', event.target.checked)} />保留临时文件（仅用于故障排查）</label>
            </div>}
          </section>
        </div>}

        {activeSection === 'maintenance' && <div id="settings-maintenance" role="tabpanel" aria-labelledby="settings-tab-maintenance" className="settings-page settings-maintenance-page">
          <section className="settings-group">
            <div className="settings-row settings-row-control"><div><strong>运行环境</strong><span>{environment ? `FFmpeg ${environment.ffmpeg ? '正常' : '未找到'} · 并发 ${environment.concurrency} · 同时任务 ${environment.max_tasks}` : '检查 FFmpeg、目录权限和当前并发设置'}</span></div><button className="secondary-button" disabled={checkingEnvironment || dirty} title={dirty ? '请先保存设置' : '检查运行环境'} onClick={checkEnvironment}><RefreshCw size={15} />{dirty ? '保存后检查' : checkingEnvironment ? '检查中…' : '检查环境'}</button></div>
            {desktopInfo?.shell && <div className="settings-row"><div><strong>桌面界面</strong><span>{desktopInfo.shell === 'tauri' ? `Tauri + React · 桌面壳 v${desktopInfo.desktop_version || '未知'}` : desktopInfo.shell}</span></div></div>}
            <div className="settings-row settings-row-control"><div><strong>软件更新</strong><span>{updateInfo ? `当前 v${updateInfo.current_version} · ${updateInfo.available ? `可更新到 v${updateInfo.latest_version}` : '已是最新版本'}` : '尚未检查'}</span></div>{updateInfo?.available && updateInfo.can_auto_install ? <button className="primary-button" disabled={installingUpdate} onClick={() => void updateApp()}><Download size={15} />{installingUpdate ? '正在下载…' : '下载安装'}</button> : <button className="secondary-button" disabled={checkingUpdate} onClick={checkUpdate}><RefreshCw size={15} />{checkingUpdate ? '检查中…' : '检查更新'}</button>}</div>
            {updateError && updateInfo?.available && <div className="inline-message update-warning" role="status">无法刷新更新信息，正在使用上次已验证的 v{updateInfo.latest_version} 信息。可以直接安装，或稍后重新检查。</div>}
            {updateError && !updateInfo?.available && <div className="inline-error settings-error" role="alert">{updateError}</div>}
            {uninstallAvailable && <div className="settings-row settings-row-control"><div><strong>卸载程序</strong><span>删除程序、设置、任务历史和缓存</span></div><button className="danger-button" onClick={uninstall}><Trash2 size={15} />卸载</button></div>}
          </section>
        </div>}
        {error && <div className="inline-error settings-error" role="alert">{error}</div>}
      </div>
      <footer><span className="settings-save-note">{dirty ? '有未保存的下载设置' : saved ? '设置已保存' : '更改主题会立即生效'}</span><Button variant="secondary" className="secondary-button" onClick={requestClose}>关闭</Button><Button className="primary-button" disabled={!dirty || saving} onClick={doSave}>{saving ? '保存中…' : saved ? '已保存' : '保存设置'}</Button></footer>
    </section>
    {showPicker && <FolderPicker initialPath={settings.download_dir || ''} onSelect={path => { update('download_dir', path); setShowPicker(false) }} onClose={() => setShowPicker(false)} />}
    {showTempPicker && <FolderPicker initialPath={settings.temp_dir || ''} onSelect={path => { update('temp_dir', path); setShowTempPicker(false) }} onClose={() => setShowTempPicker(false)} />}
    {confirmAction === 'close' && <ConfirmDialog title="放弃未保存的设置？" message="关闭后，本次修改不会生效。" confirmLabel="放弃修改" danger onCancel={() => setConfirmAction(null)} onConfirm={onClose} />}
    {confirmAction === 'update' && updateInfo && <ConfirmDialog title={`安装 v${updateInfo.latest_version}？`} message="安装包下载完成后，下载器会自动关闭并启动安装程序。" confirmLabel="下载安装" onCancel={() => setConfirmAction(null)} onConfirm={() => { setConfirmAction(null); void updateApp(true) }} />}
  </div>
}
