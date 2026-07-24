import { useState } from 'react'
import { createBatch } from '../api'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'
import { Button, Field, Input, Textarea } from './ui'

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
      // Batch is an explicit multi-add intent: allow duplicates by default.
      await createBatch(urls.map(url => ({ url, referer, concurrency, allow_duplicate: true })))
      setText(''); onAdded()
    } catch (reason: any) {
      setError(reason.message || '批量添加失败')
    } finally {
      setLoading(false)
    }
  }

  return <div className="batch-form">
    <Field label="链接列表" htmlFor="batch-urls" help="每行一个 HTTP(S) 文件、m3u8、mpd 或 magnet 链接。">
      <Textarea id="batch-urls" autoFocus placeholder={"https://example.com/a.m3u8\nhttps://example.com/file.mp4\nmagnet:?xt=urn:btih:..."} value={text} onChange={event => setText(event.target.value)} />
    </Field>
    <div className="batch-options">
      <div className="batch-referer">
        <label htmlFor="batch-referer">Referer（可选）</label>
        <Input id="batch-referer" placeholder={REQUEST_EXAMPLES.referer} value={referer} onChange={event => setReferer(event.target.value)} />
        <small>{REQUEST_FIELD_HELP.referer}</small>
      </div>
      <label htmlFor="batch-concurrency">并发</label>
      <Input id="batch-concurrency" className="number-input" type="number" min={1} max={256} value={concurrency} onChange={event => setConcurrency(Math.max(1, Math.min(256, Number(event.target.value))))} />
      <Button className="primary-button" onClick={() => void submit()} disabled={loading || !urls.length}>{loading ? '添加中...' : `添加 ${urls.length} 项`}</Button>
    </div>
    {error && <div className="inline-error" role="alert">{error}</div>}
  </div>
}
