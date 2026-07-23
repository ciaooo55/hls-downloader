import { useState } from 'react'
import { createBatch } from '../api'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'

export default function BatchAddPanel({ settings, onAdded }: { settings: any; onAdded: () => void }) {
  const [text, setText] = useState('')
  const [referer, setReferer] = useState(settings?.default_referer || '')
  const [concurrency, setConcurrency] = useState(settings?.default_concurrency || 12)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const urls = text.split('\n').map(value => value.trim()).filter(Boolean)

  const submit = async () => {
    if (!urls.length) return
    setLoading(true); setError('')
    try {
      await createBatch(urls.map(url => ({ url, referer, concurrency })))
      setText(''); onAdded()
    } catch (reason: any) {
      setError(reason.message || '批量添加失败')
    } finally {
      setLoading(false)
    }
  }

  return <div className="batch-form">
    <textarea autoFocus placeholder="每行一个 m3u8 链接" value={text} onChange={event => setText(event.target.value)} />
    <div className="batch-options"><div className="batch-referer"><label>Referer（可选）</label><input placeholder={REQUEST_EXAMPLES.referer} value={referer} onChange={event => setReferer(event.target.value)} /><small>{REQUEST_FIELD_HELP.referer}</small></div><label>并发</label><input className="number-input" type="number" min={1} max={256} value={concurrency} onChange={event => setConcurrency(Math.max(1, Math.min(256, Number(event.target.value))))} /><button className="primary-button" onClick={submit} disabled={loading || !urls.length}>{loading ? '添加中...' : `添加 ${urls.length} 项`}</button></div>
    {error && <div className="inline-error">{error}</div>}
  </div>
}
