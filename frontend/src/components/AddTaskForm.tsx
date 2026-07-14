import React, { useEffect, useState, useRef } from 'react'
import { createTask } from '../api'
import { LEGACY_REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'

export default function AddTaskForm({ settings, onAdded }: { settings: any; onAdded: () => void }) {
  const [url, setUrl] = useState('')
  const [referer, setReferer] = useState('')
  const [origin, setOrigin] = useState('')
  const [ua, setUa] = useState('')
  const [cookie, setCookie] = useState('')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [filename, setFilename] = useState('')
  const [concurrency, setConcurrency] = useState(4)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const settingsApplied = useRef(false)

  // Apply settings defaults once when they arrive
  useEffect(() => {
    if (settings && settings.default_user_agent && !settingsApplied.current) {
      settingsApplied.current = true
      setReferer(settings.default_referer || '')
      setOrigin(settings.default_origin || '')
      setUa(settings.default_user_agent || '')
      setCookie(settings.default_cookie || '')
      setConcurrency(settings.default_concurrency || 4)
    }
  }, [settings])

  useEffect(() => {
    if (!url) { setFilename(''); return }
    try {
      const u = new URL(url)
      const segs = u.pathname.split('/').filter(Boolean)
      const last = segs[segs.length - 1] || ''
      const base = last.replace(/\.m3u8$/i, '') || u.hostname
      setFilename(base)
    } catch {}
  }, [url])

  const submit = async () => {
    if (!url.trim()) return
    setLoading(true)
    setError('')
    try {
      await createTask({ url, referer, origin, user_agent: ua, cookie, filename, concurrency })
      setUrl(''); setFilename('')
      onAdded()
    } catch (e: any) {
      setError(e.message || 'network error')
    } finally { setLoading(false) }
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !loading && url.trim()) submit()
  }

  return (
    <div style={card}>
      <div style={{ display: 'flex', gap: 8, marginBottom: 10 }}>
        <input
          placeholder="粘贴 m3u8 链接，回车或点按钮添加..."
          value={url}
          onChange={e => setUrl(e.target.value)}
          onKeyDown={handleKeyDown}
          style={{ ...input, flex: 1, marginBottom: 0, fontSize: 14 }}
          autoFocus
        />
        <button onClick={submit} disabled={loading || !url.trim()} style={{ ...btn, opacity: loading || !url.trim() ? 0.5 : 1 }}>
          {loading ? '添加中...' : '添加下载'}
        </button>
      </div>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
        <input placeholder={`Referer，例如 ${LEGACY_REQUEST_EXAMPLES.referer}`} title={REQUEST_FIELD_HELP.referer} value={referer} onChange={e => setReferer(e.target.value)} style={{ ...input, flex: 2, marginBottom: 0 }} />
        <input placeholder="文件名（自动生成）" value={filename} onChange={e => setFilename(e.target.value)} style={{ ...input, flex: 2, marginBottom: 0 }} />
        <span style={{ fontSize: 11, color: '#4b5563', whiteSpace: 'nowrap' }}>并发</span>
        <input type="number" value={concurrency} onChange={e => setConcurrency(Math.max(1, +e.target.value))} style={{ ...input, width: 60, marginBottom: 0, textAlign: 'center' }} />
      </div>

      {error && (
        <div style={{ marginTop: 6, padding: '6px 10px', background: '#1a1114', border: '1px solid #3b1c24', borderRadius: 6, fontSize: 12, color: '#f87171' }}>
          添加失败：{error}
        </div>
      )}

      <div style={{ marginTop: 8 }}>
        <button onClick={() => setShowAdvanced(v => !v)} style={{ background: 'none', border: 'none', color: '#4b5563', fontSize: 11 }}>
          {showAdvanced ? '▼ 隐藏高级选项' : '▶ 高级选项（Origin / UA / Cookie）'}
        </button>
      </div>

      {showAdvanced && (
        <div style={{ display: 'flex', gap: 8, marginTop: 6, flexWrap: 'wrap' }}>
          <input placeholder={`Origin，例如 ${LEGACY_REQUEST_EXAMPLES.origin}`} title={REQUEST_FIELD_HELP.origin} value={origin} onChange={e => setOrigin(e.target.value)} style={{ ...input, flex: 1, marginBottom: 0 }} />
          <input placeholder="User-Agent（通常保持默认）" title={REQUEST_FIELD_HELP.userAgent} value={ua} onChange={e => setUa(e.target.value)} style={{ ...input, flex: 2, marginBottom: 0 }} />
          <input placeholder="Cookie，例如 sessionid=abc" title={REQUEST_FIELD_HELP.cookie} value={cookie} onChange={e => setCookie(e.target.value)} style={{ ...input, flex: 1, marginBottom: 0 }} />
        </div>
      )}
    </div>
  )
}

const card: React.CSSProperties = { background: '#1c1f26', padding: 14, borderRadius: 10, border: '1px solid #333', marginBottom: 12 }
const input: React.CSSProperties = { padding: '8px 12px', borderRadius: 6, border: '1px solid #333', background: '#0f1117', color: '#e1e4e8', fontSize: 13, marginBottom: 8 }
const btn: React.CSSProperties = { padding: '8px 20px', borderRadius: 6, border: 'none', background: '#3b82f6', color: '#fff', cursor: 'pointer', fontWeight: 700, fontSize: 14, whiteSpace: 'nowrap' }
