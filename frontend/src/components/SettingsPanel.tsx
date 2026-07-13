import { useEffect, useState } from 'react'
import { ChevronDown, ChevronRight, Copy, FolderOpen, X } from 'lucide-react'
import { fetchSettings, openExplorer, saveSettings } from '../api'
import FolderPicker from './FolderPicker'

export default function SettingsPanel({ onClose }: { onClose: () => void }) {
  const [settings, setSettings] = useState<any>({})
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState('')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [showPicker, setShowPicker] = useState(false)

  useEffect(() => {
    fetchSettings().then(setSettings).catch(reason => setError(reason.message || '加载设置失败'))
  }, [])

  const update = (key: string, value: unknown) => setSettings((current: any) => ({ ...current, [key]: value }))
  const doSave = async () => {
    setError('')
    try {
      await saveSettings(settings)
      localStorage.setItem('hls_token', settings.token || '55555')
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2000)
    } catch (reason: any) {
      setError(reason.message || '保存设置失败')
    }
  }
  const copyToken = async () => {
    try {
      await navigator.clipboard.writeText(settings.token || '')
      setSaved(true)
      window.setTimeout(() => setSaved(false), 1200)
    } catch {
      setError('无法复制，请手动选择 Token')
    }
  }

  return <div className="modal-overlay" onMouseDown={onClose}>
    <section className="modal settings-modal" onMouseDown={event => event.stopPropagation()}>
      <header><div><h2>设置</h2><p>下载目录、并发与请求参数</p></div><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></header>

      <label>Token（油猴脚本自动使用此值）</label>
      <div className="input-action"><input value={settings.token || ''} readOnly /><button className="secondary-button" title="复制 Token" onClick={copyToken}><Copy size={15} />复制</button></div>

      <label>下载保存目录</label>
      <div className="input-action"><input value={settings.download_dir || ''} onChange={event => update('download_dir', event.target.value)} /><button className="secondary-button" onClick={() => setShowPicker(true)}>选择目录</button><button className="icon-button bordered" title="打开目录" onClick={() => openExplorer(settings.download_dir || '')}><FolderOpen size={17} /></button></div>
      <p className="field-note">临时分片保存在下载目录的 .tasks 子目录，完成后按设置清理。</p>

      <div className="form-row">
        <div><label>默认并发数</label><input type="number" min={1} max={64} value={settings.default_concurrency ?? 4} onChange={event => update('default_concurrency', Number(event.target.value))} /></div>
        <div><label>最大同时任务数</label><input type="number" min={1} max={16} value={settings.max_concurrent_tasks ?? 2} onChange={event => update('max_concurrent_tasks', Number(event.target.value))} /></div>
      </div>
      <label>默认 Referer</label><input value={settings.default_referer || ''} onChange={event => update('default_referer', event.target.value)} />

      <button className="text-button advanced-toggle" onClick={() => setShowAdvanced(value => !value)}>{showAdvanced ? <ChevronDown size={14} /> : <ChevronRight size={14} />}高级选项</button>
      {showAdvanced && <div className="advanced-settings">
        <label>默认 Origin</label><input value={settings.default_origin || ''} onChange={event => update('default_origin', event.target.value)} />
        <label>默认 User-Agent</label><input value={settings.default_user_agent || ''} onChange={event => update('default_user_agent', event.target.value)} />
        <label>默认 Cookie</label><input value={settings.default_cookie || ''} onChange={event => update('default_cookie', event.target.value)} />
        <label>ffmpeg 路径</label><div className="input-action"><input value={settings.ffmpeg_path || ''} onChange={event => update('ffmpeg_path', event.target.value)} /><button className="icon-button bordered" title="打开文件位置" onClick={() => openExplorer(settings.ffmpeg_path || '')}><FolderOpen size={17} /></button></div>
        <label>允许的域名（逗号分隔，留空表示不限）</label><input value={(settings.allowed_hosts || []).join(',')} onChange={event => update('allowed_hosts', event.target.value.split(',').map(value => value.trim()).filter(Boolean))} />
        <label className="checkbox-label"><input type="checkbox" checked={settings.keep_temp_files || false} onChange={event => update('keep_temp_files', event.target.checked)} />保留临时文件（调试用）</label>
      </div>}

      {error && <div className="inline-error">{error}</div>}
      <footer><button className="secondary-button" onClick={onClose}>关闭</button><button className="primary-button" onClick={doSave}>{saved ? '已完成' : '保存设置'}</button></footer>
    </section>
    {showPicker && <FolderPicker initialPath={settings.download_dir || ''} onSelect={path => { update('download_dir', path); setShowPicker(false) }} onClose={() => setShowPicker(false)} />}
  </div>
}
