import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import { ChevronDown, ChevronRight, Download, FolderOpen, RefreshCw, ShieldCheck, Trash2, X } from 'lucide-react'
import { fetchLegalStatus, fetchSettings, fetchUpdateInfo, installUpdate, openExplorer, saveSettings, scanCastDevices, scanTvboxDevices, testConnection } from '../api'
import { beginUninstall, getDesktopInfo } from '../desktop'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'
import type { ThemePreference } from '../theme'
import type { LegalStatus, UpdateInfo } from '../types'
import { friendlyUpdateError } from '../updateError'
import { playCompletionChime } from '../completionSound'

const QUEUE_DAY_LABELS = ['周一', '周二', '周三', '周四', '周五', '周六', '周日']
import { pickFolder } from '../desktop'
import FolderPicker from './FolderPicker'
import SiteProfilesEditor from './SiteProfilesEditor'
import ConfirmDialog from './ConfirmDialog'
import LegalAgreementDialog from './LegalAgreementDialog'
import { Button } from './ui'
import { DOWNLOAD_CATEGORY_LABELS, type DownloadCategory } from '../downloadCategory'

type SettingsSection = 'general' | 'network' | 'maintenance'
const SETTINGS_SECTIONS: SettingsSection[] = ['general', 'network', 'maintenance']
const SECRET_MASK = '••••••••'

export default function SettingsPanel({ themePreference, onThemePreferenceChange, onClose }: {
  themePreference: ThemePreference
  onThemePreferenceChange: (theme: ThemePreference) => void
  onClose: () => void
}) {
  const [settings, setSettings] = useState<any>({})
  const [original, setOriginal] = useState<any>({})
  const [saved, setSaved] = useState(false)
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
  const [castDevices, setCastDevices] = useState<Array<{ id: string; protocol: 'dlna' | 'chromecast'; location: string; control_url: string; service_type: string; label: string; host: string }>>([])
  const [scanningCast, setScanningCast] = useState(false)
  const [castScanMessage, setCastScanMessage] = useState('')
  const [activeSection, setActiveSection] = useState<SettingsSection>('general')
  const [legalStatus, setLegalStatus] = useState<LegalStatus | null>(null)
  const [showLegal, setShowLegal] = useState(false)
  const dialogRef = useRef<HTMLElement>(null)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const previousFocusRef = useRef<HTMLElement | null>(null)
  const dirty = JSON.stringify(settings) !== JSON.stringify(original)
  const selectedTvboxDevice = tvboxDevices.some(device => device.endpoint === settings.tvbox_endpoint)
  const tvboxSelectValue = settings.tvbox_endpoint
    ? (selectedTvboxDevice ? settings.tvbox_endpoint : '__manual__')
    : ''
  const selectedCastDevice = castDevices.find(device => device.id === settings.cast_device?.id)
  const castSelectValue = settings.cast_device?.id
    ? (selectedCastDevice ? settings.cast_device.id : '__saved__')
    : ''

  useEffect(() => {
    fetchSettings().then(data => {
      const editable = {
        ...data,
        default_cookie: data.default_cookie_configured ? SECRET_MASK : '',
      }
      setSettings(editable); setOriginal(editable)
    }).catch(reason => setError(reason.message || '加载设置失败'))
    getDesktopInfo().then(info => { setUninstallAvailable(info.installed === true); setDesktopInfo(info) })
    fetchUpdateInfo().then(setUpdateInfo).catch(() => {})
    fetchLegalStatus().then(setLegalStatus).catch(() => {})
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
    if (installingUpdate) return
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
  }, [dirty, showPicker, showTempPicker, confirmAction, installingUpdate])

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
    if (settings.default_concurrency < 1 || settings.default_concurrency > 64) { setError('默认并发数必须在 1 到 64 之间'); return }
    if (settings.max_concurrent_tasks < 1 || settings.max_concurrent_tasks > 16) { setError('最大同时任务数必须在 1 到 16 之间'); return }
    if (settings.http_chunk_size_mb < 1 || settings.http_chunk_size_mb > 64) { setError('HTTP 分段大小必须在 1 到 64 MiB 之间'); return }
    if (settings.download_speed_limit_kib != null && settings.download_speed_limit_kib < 0) { setError('下载限速不能小于 0'); return }
    if (settings.speed_schedule_enabled) {
      if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(String(settings.speed_schedule_start || ''))) { setError('分时段开始时间必须为 HH:MM'); return }
      if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(String(settings.speed_schedule_end || ''))) { setError('分时段结束时间必须为 HH:MM'); return }
      if (settings.speed_schedule_limit_kib != null && settings.speed_schedule_limit_kib < 0) { setError('分时段限速不能小于 0'); return }
    }
    if (settings.live_record_max_minutes != null && (settings.live_record_max_minutes < 0 || settings.live_record_max_minutes > 2880)) { setError('直播录制时长上限必须在 0 到 2880 分钟之间'); return }
    if (settings.proxy_mode === 'manual' && !String(settings.proxy_url || '').trim()) { setError('手动代理模式必须填写代理地址'); setActiveSection('network'); return }
    if (settings.bt_upload_limit_kib < 0) { setError('BT 上传限制不能小于 0'); return }
    const avCommand = String(settings.av_scan_command || '').trim()
    if (avCommand && !avCommand.includes('{file}')) { setError('自定义扫描命令必须包含 {file}'); return }
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
      const normalized = await saveSettings({ ...settings, site_profiles: (settings.site_profiles || []).filter((item: { host?: string }) => String(item?.host || "").trim()) })
      const editable = {
        ...normalized,
        default_cookie: normalized.default_cookie_configured ? SECRET_MASK : '',
      }
      setSettings(editable); setOriginal(editable)
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2000)
    } catch (reason: any) {
      setError(reason.message || '保存设置失败')
    } finally { setSaving(false) }
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
  const scanCast = async () => {
    setScanningCast(true); setError(''); setCastScanMessage('正在搜索支持 DLNA 的播放设备…')
    try {
      const result = await scanCastDevices()
      const devices = result.devices || []
      setCastDevices(devices)
      setCastScanMessage(devices.length ? `发现 ${devices.length} 台投屏设备，请选择后保存设置。` : '未发现投屏设备。请确认电视已开启 DLNA/媒体渲染且和电脑处于同一局域网。')
    } catch (reason: any) {
      setCastScanMessage(reason.message || '搜索投屏设备失败')
    } finally { setScanningCast(false) }
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

  return <div className="modal-overlay settings-overlay" onMouseDown={() => { if (!installingUpdate) requestClose() }}>
    <section ref={dialogRef} className="modal settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-dialog-title" onMouseDown={event => event.stopPropagation()}>
      <header className="settings-header">
        <div className="settings-title"><h2 id="settings-dialog-title">应用设置{dirty ? ' *' : ''}</h2><p>界面、下载行为与运行环境</p></div>
        <nav className="settings-tabs" role="tablist" aria-label="设置分区" onKeyDown={moveSettingsTab}>
          <button id="settings-tab-general" type="button" role="tab" aria-selected={activeSection === 'general'} aria-controls="settings-general" className={activeSection === 'general' ? 'active' : ''} onClick={() => setActiveSection('general')}>通用</button>
          <button id="settings-tab-network" type="button" role="tab" aria-selected={activeSection === 'network'} aria-controls="settings-network" className={activeSection === 'network' ? 'active' : ''} onClick={() => setActiveSection('network')}>网络与下载</button>
          <button id="settings-tab-maintenance" type="button" role="tab" aria-selected={activeSection === 'maintenance'} aria-controls="settings-maintenance" className={activeSection === 'maintenance' ? 'active' : ''} onClick={() => setActiveSection('maintenance')}>维护</button>
        </nav>
        <Button ref={closeButtonRef} variant="ghost" size="icon" className="icon-button settings-close" title={installingUpdate ? '正在安装更新' : '关闭'} aria-label="关闭" disabled={installingUpdate} onClick={requestClose}><X size={18} /></Button>
      </header>
      {installingUpdate && <div className="settings-installing" role="status">正在下载并启动安装程序，请勿关闭设置窗口。</div>}
      <div className="settings-body">
        {activeSection === 'general' && <div id="settings-general" role="tabpanel" aria-labelledby="settings-tab-general" className="settings-page">
          <section className="settings-group">
            <div className="settings-row settings-row-control">
              <div><strong>应用主题</strong><span>跟随系统，或固定使用浅色/深色外观</span></div>
              <select aria-label="应用主题" value={themePreference} onChange={event => onThemePreferenceChange(event.target.value as ThemePreference)}>
                <option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option>
              </select>
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
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={Boolean(settings.auto_category_dirs)} onChange={event => update('auto_category_dirs', event.target.checked)} />按文件类型自动分类到子目录</label>
            <p className="field-note">关闭时全部进入上方下载目录。开启后，未单独指定分类目录的任务会保存到“媒体 / 程序 / 压缩包 / 其他”。任务里手动选择的目录不会被改走。</p>
            {(['media', 'program', 'archive', 'other'] as DownloadCategory[]).map(category => (
              <div className="settings-row" key={category}>
                <div><strong>{DOWNLOAD_CATEGORY_LABELS[category]}</strong><span>可覆盖该类型的默认分类目录</span></div>
                <div className="input-action"><input aria-label={`${DOWNLOAD_CATEGORY_LABELS[category]}保存目录`} value={settings.browser_category_dirs?.[category] || ''} placeholder={settings.auto_category_dirs ? `默认：下载目录\\${DOWNLOAD_CATEGORY_LABELS[category]}` : '使用下载保存目录'} onChange={event => update('browser_category_dirs', { ...(settings.browser_category_dirs || {}), [category]: event.target.value })} /><button className="secondary-button" onClick={() => void (async () => {
                  const native = await pickFolder(settings.browser_category_dirs?.[category] || settings.download_dir || '')
                  if (native.ok && native.path) update('browser_category_dirs', { ...(settings.browser_category_dirs || {}), [category]: native.path })
                })()}>选择目录</button><button className="icon-button bordered" title="打开目录" onClick={() => openExplorer(settings.browser_category_dirs?.[category] || settings.download_dir || '')}><FolderOpen size={17} /></button></div>
              </div>
            ))}
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
            <div className="settings-field"><label htmlFor="setting-default-concurrency">默认并发数</label><input id="setting-default-concurrency" type="number" min={1} max={64} value={settings.default_concurrency ?? 12} onChange={event => update('default_concurrency', Number(event.target.value))} /><p>{REQUEST_FIELD_HELP.concurrency}</p></div>
            <div className="settings-field"><label htmlFor="setting-max-tasks">最大同时任务数</label><input id="setting-max-tasks" type="number" min={1} max={16} value={settings.max_concurrent_tasks ?? 3} onChange={event => update('max_concurrent_tasks', Number(event.target.value))} /><p>{REQUEST_FIELD_HELP.maxTasks}</p></div>
            <div className="settings-field"><label htmlFor="setting-speed-limit">全局下载限速（KiB/s）</label><input id="setting-speed-limit" type="number" min={0} max={1048576} value={settings.download_speed_limit_kib ?? 0} onChange={event => update('download_speed_limit_kib', Number(event.target.value))} /><p>{REQUEST_FIELD_HELP.speedLimit}</p></div>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.speed_schedule_enabled ?? false} onChange={event => update('speed_schedule_enabled', event.target.checked)} />分时段限速</label>
            <div className="settings-field"><label htmlFor="setting-speed-schedule-start">时段开始</label><input id="setting-speed-schedule-start" type="time" disabled={!settings.speed_schedule_enabled} value={settings.speed_schedule_start ?? '08:00'} onChange={event => update('speed_schedule_start', String(event.target.value).slice(0, 5))} /><p>开始晚于结束时按跨午夜处理，与定时队列相同。</p></div>
            <div className="settings-field"><label htmlFor="setting-speed-schedule-end">时段结束</label><input id="setting-speed-schedule-end" type="time" disabled={!settings.speed_schedule_enabled} value={settings.speed_schedule_end ?? '23:00'} onChange={event => update('speed_schedule_end', String(event.target.value).slice(0, 5))} /><p>时段为半开区间 [start, end)。</p></div>
            <div className="settings-field"><label htmlFor="setting-speed-schedule-limit">时段限速（KiB/s）</label><input id="setting-speed-schedule-limit" type="number" min={0} max={1048576} disabled={!settings.speed_schedule_enabled} value={settings.speed_schedule_limit_kib ?? 0} onChange={event => update('speed_schedule_limit_kib', Number(event.target.value))} /><p>0 表示该时段不限速。关闭后仍使用上方的全局限速。</p></div>
            <div className="settings-field"><label htmlFor="setting-exist-policy">同名文件</label><select id="setting-exist-policy" value={settings.existing_file_policy || 'rename'} onChange={event => update('existing_file_policy', event.target.value)}><option value="rename">自动重命名</option><option value="overwrite">覆盖</option><option value="skip">跳过</option></select><p>默认与之前一样自动加 _1。覆盖会替换已有文件；跳过则让任务失败并保留原文件。</p></div><div className="settings-field"><label htmlFor="setting-http-chunk">HTTP 分段大小（MiB）</label><input id="setting-http-chunk" type="number" min={1} max={64} value={settings.http_chunk_size_mb ?? 8} onChange={event => update('http_chunk_size_mb', Number(event.target.value))} /><p>每段完成后可安全暂停；较小更灵活，较大请求更少。</p></div>
            <div className="settings-field"><label htmlFor="setting-live-max">直播录制时长上限（分钟）</label><input id="setting-live-max" type="number" min={0} max={2880} value={settings.live_record_max_minutes ?? 0} onChange={event => update('live_record_max_minutes', Number(event.target.value))} /><p>录制直播 HLS 达到该时长后自动停止并合并；0 表示不限制，随时可手动停止录制。</p></div>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.download_subtitles ?? true} onChange={event => update('download_subtitles', event.target.checked)} />下载 HLS 外挂字幕（保存为 .vtt / .srt）</label>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.skip_ad_segments ?? true} onChange={event => update('skip_ad_segments', event.target.checked)} />跳过 HLS 明确标记的广告分片</label>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.clipboard_watch ?? true} onChange={event => update('clipboard_watch', event.target.checked)} />监视剪贴板中的下载链接（仅桌面版）</label>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={Boolean(settings.completion_sound_enabled)} onChange={event => update('completion_sound_enabled', event.target.checked)} />下载完成时播放提示音（默认关闭）</label>
            <div className="settings-field"><button type="button" className="secondary-button" onClick={() => playCompletionChime(true)}>试听提示音</button><p>与系统通知独立；短时间内多个任务完成只响一次。</p></div>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={Boolean(settings.av_scan_enabled)} onChange={event => update('av_scan_enabled', event.target.checked)} />下载完成后扫描病毒（默认关闭）</label>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.av_scan_fail_on_threat ?? true} disabled={!settings.av_scan_enabled} onChange={event => update('av_scan_fail_on_threat', event.target.checked)} />发现威胁时将任务标为失败（不删除文件）</label>
            <div className="settings-field settings-field-wide"><label htmlFor="setting-av-cmd">自定义扫描命令</label><input id="setting-av-cmd" disabled={!settings.av_scan_enabled} value={settings.av_scan_command || ''} onChange={event => update('av_scan_command', event.target.value)} placeholder='"C:\Program Files\ClamAV\clamscan.exe" --no-summary {file}' /><p>留空时优先使用 Windows Defender；自定义命令必须包含 {'{file}'}。找不到扫描器时仍保留下载结果。</p></div>
          </section>
          <h3 className="settings-group-label">定时队列</h3>
          <section className="settings-group settings-grid-group">
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={Boolean(settings.resume_interrupted_on_startup)} onChange={event => update('resume_interrupted_on_startup', event.target.checked)} />启动时自动恢复上次中断的下载（默认关闭）</label>
            <div className="settings-field"><label htmlFor="setting-auto-retry-max">失败后自动重试次数</label><input id="setting-auto-retry-max" type="number" min={0} max={10} value={settings.auto_retry_failed_max ?? 0} onChange={event => update('auto_retry_failed_max', Math.max(0, Math.min(10, Number(event.target.value) || 0)))} /><p>0 表示关闭。只重试网络/5xx 等瞬时失败，不重试 403/校验失败/病毒结果。默认关闭。</p></div>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.queue_auto_start_enabled ?? false} onChange={event => update('queue_auto_start_enabled', event.target.checked)} />在指定时间自动开始新队列</label>
            <div className="settings-field"><label htmlFor="setting-queue-auto-start">自动开始时间</label><input id="setting-queue-auto-start" type="time" disabled={!settings.queue_auto_start_enabled} value={settings.queue_auto_start_time ?? '00:00'} onChange={event => update('queue_auto_start_time', event.target.value)} /><p>开启后，新任务保持排队，直到当天该时间开始。排队中可右键调整优先级（上移/下移/队首/队尾）。</p></div>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.queue_auto_stop_enabled ?? false} onChange={event => update('queue_auto_stop_enabled', event.target.checked)} />在指定时间自动停止队列</label>
            <div className="settings-field"><label htmlFor="setting-queue-auto-stop">自动停止时间</label><input id="setting-queue-auto-stop" type="time" disabled={!settings.queue_auto_stop_enabled} value={settings.queue_auto_stop_time ?? '07:30'} onChange={event => update('queue_auto_stop_time', event.target.value)} /><p>普通下载会安全暂停，直播会停止录制并合并；跨午夜时间段也能正确处理。</p></div>
            <div className="settings-field settings-field-wide"><label>队列生效星期</label><div className="settings-day-picker">{QUEUE_DAY_LABELS.map((label, day) => { const selected: number[] = settings.queue_active_days ?? [0, 1, 2, 3, 4, 5, 6]; return <label className="checkbox-label" key={label}><input type="checkbox" checked={selected.includes(day)} onChange={event => { const next = event.target.checked ? [...selected, day].sort() : selected.filter((value: number) => value !== day); if (next.length) update('queue_active_days', next) }} />{label}</label> })}</div><p>至少保留一天；开始时间晚于停止时间时按跨午夜队列处理。</p></div>
          </section>
        </div>}

        {activeSection === 'network' && <div id="settings-network" role="tabpanel" aria-labelledby="settings-tab-network" className="settings-page">
          <h3 className="settings-group-label settings-group-label-first">代理</h3>
          <section className="settings-group settings-grid-group">
            <div className="settings-field"><label htmlFor="setting-proxy-mode">连接方式</label><select id="setting-proxy-mode" value={settings.proxy_mode || 'system'} onChange={event => update('proxy_mode', event.target.value)}><option value="system">跟随系统代理</option><option value="direct">始终直连</option><option value="manual">手动代理</option></select><p>读取 Windows 系统代理（含 WinINET）和环境变量，应用于 HTTP、HLS 和 DASH。</p></div>
            <div className="settings-field"><label htmlFor="setting-proxy-url">代理地址</label><input id="setting-proxy-url" disabled={settings.proxy_mode !== 'manual'} value={settings.proxy_url || ''} onChange={event => update('proxy_url', event.target.value)} placeholder="http://user:pass@127.0.0.1:7890" /><p>支持 HTTP(S)、SOCKS5 与带认证的代理 URL。</p></div>
            <div className="settings-field"><label htmlFor="setting-proxy-bypass">不走代理的 Host</label><input id="setting-proxy-bypass" value={(settings.proxy_bypass || []).join(', ')} onChange={event => update('proxy_bypass', event.target.value.split(',').map(value => value.trim()).filter(Boolean))} placeholder="localhost, 127.0.0.1, *.lan" /><p>逗号分隔，支持通配符。</p></div>
          </section>

                    <h3 className="settings-group-label">按站点下载规则</h3>
          <section className="settings-group settings-grid-group">
            <div className="settings-field settings-field-wide">
              <SiteProfilesEditor value={settings.site_profiles || []} onChange={(profiles) => update('site_profiles', profiles)} />
            </div>
          </section>

          <h3 className="settings-group-label">BT 下载</h3>
          <section className="settings-group settings-grid-group">
            <div className="settings-field"><label htmlFor="setting-bt-upload">上传上限（KiB/s）</label><input id="setting-bt-upload" type="number" min={0} max={1048576} value={settings.bt_upload_limit_kib ?? 1024} onChange={event => update('bt_upload_limit_kib', Number(event.target.value))} /><p>0 表示不限速；完成后会立即停止做种。</p></div>
            <div className="settings-field"><label htmlFor="setting-bt-peers">最大 Peer 连接</label><input id="setting-bt-peers" type="number" min={10} max={1000} value={settings.bt_max_connections ?? 200} onChange={event => update('bt_max_connections', Number(event.target.value))} /><p>默认 200；种子稀少时可提高连接发现速度，网络受限时再降低。</p></div>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={settings.bt_enable_dht ?? true} onChange={event => update('bt_enable_dht', event.target.checked)} />启用 DHT 节点发现</label>
            <label className="checkbox-label settings-checkbox"><input type="checkbox" checked={Boolean(settings.watch_torrents)} onChange={event => update('watch_torrents', event.target.checked)} />监视文件夹中的新种子</label>
            <div className="settings-row">
              <div><strong>种子监视目录</strong><span>只导入开启后新放入的 .torrent / .url / .magnet；已有文件不会自动添加</span></div>
              <div className="input-action"><input aria-label="种子监视目录" value={settings.watch_dir || ''} onChange={event => update('watch_dir', event.target.value)} placeholder="选择一个专门放种子的文件夹" /><button className="secondary-button" onClick={() => void (async () => {
                const native = await pickFolder(settings.watch_dir || settings.download_dir || '')
                if (native.ok && native.path) update('watch_dir', native.path)
              })()}>选择目录</button><button className="icon-button bordered" title="打开目录" onClick={() => openExplorer(settings.watch_dir || '')}><FolderOpen size={17} /></button></div>
            </div>

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

          <h3 className="settings-group-label">投屏（DLNA/UPnP）</h3>
          <section className="settings-group settings-grid-group">
            <div className="settings-field settings-field-wide">
              <label htmlFor="setting-cast-device">默认投屏设备</label>
              <div className="input-action">
                <select id="setting-cast-device" value={castSelectValue} onChange={event => {
                  const value = event.target.value
                  if (value === '') update('cast_device', {})
                  else {
                    const device = castDevices.find(item => item.id === value)
                    if (device) update('cast_device', device)
                  }
                }} aria-label="默认投屏设备">
                  <option value="">不使用投屏</option>
                  {castSelectValue === '__saved__' && <option value="__saved__" disabled>{settings.cast_device?.label || '已保存的投屏设备'} · {settings.cast_device?.host}</option>}
                  {castDevices.map(device => <option key={device.id} value={device.id}>{device.label} · {device.protocol === 'chromecast' ? 'Chromecast' : 'DLNA'} · {device.host}</option>)}
                </select>
                <Button variant="secondary" className="secondary-button" disabled={scanningCast} title="搜索同一局域网中的 DLNA 与 Chromecast 播放设备" onClick={() => void scanCast()}><RefreshCw size={15} />{scanningCast ? '搜索中…' : '搜索设备'}</Button>
              </div>
              <p>投屏会将选中的本机媒体文件通过临时局域网链接交给电视播放；支持 DLNA/UPnP 与 Chromecast。</p>
              {castScanMessage && <p className="settings-inline-status" role="status" aria-live="polite">{castScanMessage}</p>}
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
            <div className="settings-row settings-row-control"><div><strong>法律与隐私</strong><span>{legalStatus?.accepted ? `已接受中国大陆版 ${legalStatus.accepted_version} · 记录仅在本机` : '读取用户协议与本机接受状态'}</span></div><button className="secondary-button" disabled={!legalStatus} onClick={() => setShowLegal(true)}><ShieldCheck size={15} />查看协议</button></div>
            {updateError && updateInfo?.available && <div className="inline-message update-warning" role="status">无法刷新更新信息，正在使用上次已验证的 v{updateInfo.latest_version} 信息。可以直接安装，或稍后重新检查。</div>}
            {updateError && !updateInfo?.available && <div className="inline-error settings-error" role="alert">{updateError}</div>}
            {uninstallAvailable && <div className="settings-row settings-row-control"><div><strong>卸载程序</strong><span>删除程序、设置、任务历史和缓存</span></div><button className="danger-button" onClick={uninstall}><Trash2 size={15} />卸载</button></div>}
          </section>
        </div>}
        {error && <div className="inline-error settings-error" role="alert">{error}</div>}
      </div>
      <footer><span className="settings-save-note">{dirty ? '有未保存的下载设置' : saved ? '设置已保存' : '更改主题会立即生效'}</span><Button variant="secondary" className="secondary-button" disabled={installingUpdate} onClick={requestClose}>关闭</Button><Button className="primary-button" disabled={!dirty || saving} onClick={doSave}>{saving ? '保存中…' : saved ? '已保存' : '保存设置'}</Button></footer>
    </section>
    {showPicker && <FolderPicker initialPath={settings.download_dir || ''} onSelect={path => { update('download_dir', path); setShowPicker(false) }} onClose={() => setShowPicker(false)} />}
    {showTempPicker && <FolderPicker initialPath={settings.temp_dir || ''} onSelect={path => { update('temp_dir', path); setShowTempPicker(false) }} onClose={() => setShowTempPicker(false)} />}
    {confirmAction === 'close' && <ConfirmDialog title="放弃未保存的设置？" message="关闭后，本次修改不会生效。" confirmLabel="放弃修改" danger onCancel={() => setConfirmAction(null)} onConfirm={onClose} />}
    {confirmAction === 'update' && updateInfo && <ConfirmDialog title={`安装 v${updateInfo.latest_version}？`} message={updateInfo.asset_kind === 'portable' ? '便携更新包下载并校验后，下载器会自动关闭、事务式替换程序文件并重新启动；配置、任务和下载文件会保留。' : '安装包下载完成并校验后，下载器会自动关闭并启动安装程序。'} confirmLabel="下载安装" onCancel={() => setConfirmAction(null)} onConfirm={() => { setConfirmAction(null); void updateApp(true) }} />}
    {showLegal && <LegalAgreementDialog status={legalStatus} required={false} onClose={() => setShowLegal(false)} />}
  </div>
}
