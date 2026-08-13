import { useMemo, useState } from 'react'
import { createBatch, harvestPage, harvestPageProbe } from '../api'
import { parseUrlList, URL_LIST_LIMIT } from '../urlList'
import { REQUEST_EXAMPLES, REQUEST_FIELD_HELP } from '../requestHelp'
import { HARVEST_FILTERS, applyHarvestProbes, filterHarvestLinks, filterHarvestLinksByMinSize, harvestFilterCounts, type HarvestCategory, type HarvestLink } from '../pageHarvest'
import { fmtBytes } from '../format'
import { Button, Field, Input, Textarea } from './ui'

export default function BatchAddPanel({ settings, onAdded, initialText = '', initialMode = 'list' }: { settings: any; onAdded: () => void; initialText?: string; initialMode?: 'list' | 'harvest' }) {
  const [mode, setMode] = useState<'list' | 'harvest'>(initialMode)
  const [text, setText] = useState(initialText)
  const [pageUrl, setPageUrl] = useState('')
  const [harvestLinks, setHarvestLinks] = useState<HarvestLink[]>([])
  const [harvestMessage, setHarvestMessage] = useState('')
  const [harvestTitle, setHarvestTitle] = useState('')
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState<'all' | HarvestCategory>('all')
  const [minSize, setMinSize] = useState(0)
  const [probing, setProbing] = useState(false)
  const [probed, setProbed] = useState(false)
  const [referer, setReferer] = useState(settings?.default_referer || '')
  const [concurrency, setConcurrency] = useState(settings?.default_concurrency || 12)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const parsed = parseUrlList(text)
  const urls = parsed.urls
  const visibleLinks = useMemo(() => filterHarvestLinksByMinSize(filterHarvestLinks(harvestLinks, filter), minSize), [harvestLinks, filter, minSize])
  const counts = useMemo(() => harvestFilterCounts(harvestLinks), [harvestLinks])
  const selectedUrls = visibleLinks.filter(item => selected.has(item.url)).map(item => item.url)
  const addCount = mode === 'harvest' ? selectedUrls.length : urls.length

  const harvest = async () => {
    const value = pageUrl.trim()
    if (!value) { setError('请输入要抓取的网页地址'); return }
    setLoading(true); setError(''); setHarvestLinks([]); setSelected(new Set()); setHarvestMessage(''); setProbed(false); setMinSize(0)
    try {
      const found = await harvestPage({ url: value, referer: referer || value })
      const links = Array.isArray(found?.links) ? found.links as HarvestLink[] : []
      setHarvestLinks(links)
      setHarvestTitle(String(found?.title || ''))
      setHarvestMessage(String(found?.message || ''))
      setSelected(new Set(links.map(item => item.url)))
      if (!referer) setReferer(String(found?.final_url || value))
      if (!links.length) setError(String(found?.message || '页面未发现可下载链接'))
    } catch (reason: any) {
      setError(reason.message || '页面抓取失败')
    } finally {
      setLoading(false)
    }
  }

  const probeSizes = async () => {
    if (!harvestLinks.length) return
    setProbing(true); setError('')
    try {
      const found = await harvestPageProbe({ urls: harvestLinks.map(item => item.url), referer: referer || pageUrl })
      setHarvestLinks(current => applyHarvestProbes(current, found?.probes || []))
      setProbed(true)
    } catch (reason: any) {
      setError(reason.message || '读取文件大小失败')
    } finally {
      setProbing(false)
    }
  }

  const toggleAll = (on: boolean) => {
    setSelected(current => {
      const next = new Set(current)
      for (const item of visibleLinks) {
        if (on) next.add(item.url)
        else next.delete(item.url)
      }
      return next
    })
  }

  const submit = async () => {
    const chosen = mode === 'harvest' ? selectedUrls : urls
    if (!chosen.length) { setError(mode === 'harvest' ? '请先勾选要添加的链接' : '没有识别到 HTTP(S) 或 magnet 链接'); return }
    setLoading(true); setError('')
    try {
      await createBatch(chosen.map(url => ({ url, referer: referer || pageUrl, concurrency, allow_duplicate: true })))
      setText(''); setHarvestLinks([]); setSelected(new Set()); onAdded()
    } catch (reason: any) {
      setError(reason.message || '批量添加失败')
    } finally {
      setLoading(false)
    }
  }

  return <div className="batch-form">
    <div className="batch-mode-tabs" role="tablist" aria-label="添加方式">
      <button type="button" role="tab" aria-selected={mode === 'list'} className={mode === 'list' ? 'is-active' : ''} onClick={() => { setMode('list'); setError('') }}>粘贴列表</button>
      <button type="button" role="tab" aria-selected={mode === 'harvest'} className={mode === 'harvest' ? 'is-active' : ''} onClick={() => { setMode('harvest'); setError('') }}>网页抓取</button>
    </div>
    {mode === 'list' ? (
      <>
        <Field label="链接列表" htmlFor="batch-urls" help="可粘贴杂乱文本、HTML 或从文件导入。自动提取 HTTP(S)/magnet，忽略注释和重复项，最多 100 条。">
          <Textarea id="batch-urls" autoFocus placeholder={"https://example.com/a.m3u8\nhttps://example.com/file.mp4\nftp://nas.example.test/pub/file.bin\nsftp://nas.example.test/pub/file.bin\nmagnet:?xt=urn:btih:..."} value={text} onChange={event => setText(event.target.value)} />
        </Field>
        <div className="batch-file-row">
          <input id="batch-file" type="file" accept=".txt,.html,.htm,.url,.csv,.json" hidden onChange={event => {
            const file = event.target.files?.[0]
            event.target.value = ''
            if (!file) return
            void file.text().then(content => {
              const imported = parseUrlList(content)
              setText(imported.urls.join('\n'))
              setError(imported.urls.length ? (imported.truncated ? `文件里超过 ${URL_LIST_LIMIT} 条，已只导入前 ${URL_LIST_LIMIT} 条` : '') : '文件里没有识别到可下载链接')
            }).catch(() => setError('无法读取该文件'))
          }} />
          <Button type="button" variant="secondary" onClick={() => document.getElementById('batch-file')?.click()}>从文件导入</Button>
          {parsed.truncated && <small>已截取前 {URL_LIST_LIMIT} 条</small>}
        </div>
      </>
    ) : (
      <>
        <Field label="网页地址" htmlFor="harvest-url" help="只读取当前这一页的 HTML，提取带扩展名的静态文件、FTP 和 magnet。不执行脚本，也不会继续打开子页面。">
          <div className="harvest-url-row">
            <Input id="harvest-url" autoFocus placeholder="https://example.com/files/" value={pageUrl} onChange={event => setPageUrl(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') void harvest() }} />
            <Button type="button" variant="secondary" disabled={loading || !pageUrl.trim()} onClick={() => void harvest()}>{loading ? '抓取中...' : '抓取本页链接'}</Button>
          </div>
        </Field>
        {harvestLinks.length > 0 && (
          <section className="harvest-results" aria-label="抓取结果">
            <div className="harvest-results-heading">
              <strong>{harvestTitle || '可下载链接'}</strong>
              <span>{harvestMessage}</span>
            </div>
            <div className="harvest-filters">
              {HARVEST_FILTERS.filter(item => item.id === 'all' || counts[item.id]).map(item => (
                <button type="button" key={item.id} className={filter === item.id ? 'is-active' : ''} onClick={() => setFilter(item.id)}>{item.label} {counts[item.id] || 0}</button>
              ))}
            </div>
            <div className="harvest-select-row">
              <button type="button" className="text-button" onClick={() => toggleAll(true)}>全选当前分类</button>
              <button type="button" className="text-button" onClick={() => toggleAll(false)}>取消当前分类</button>
              <button type="button" className="text-button" disabled={probing || loading} onClick={() => void probeSizes()}>{probing ? '读取大小...' : '读取大小'}</button>
              <button type="button" className={minSize === 0 ? 'text-button is-active' : 'text-button'} onClick={() => setMinSize(0)}>全部大小</button>
              <button type="button" className={minSize === 1048576 ? 'text-button is-active' : 'text-button'} disabled={!probed} onClick={() => setMinSize(1048576)}>&gt;= 1 MB</button>
              <button type="button" className={minSize === 10485760 ? 'text-button is-active' : 'text-button'} disabled={!probed} onClick={() => setMinSize(10485760)}>&gt;= 10 MB</button>
              <small>已选 {selectedUrls.length} / {visibleLinks.length}</small>
            </div>
            <div className="harvest-list">
              {visibleLinks.map(item => (
                <label key={item.url} className="harvest-item">
                  <input type="checkbox" checked={selected.has(item.url)} onChange={() => setSelected(current => { const next = new Set(current); if (next.has(item.url)) next.delete(item.url); else next.add(item.url); return next })} />
                  <span>
                    <strong title={item.url}>{item.label || item.filename}</strong>
                    <small>{item.filename} · {item.category}{item.extension ? ` · .${item.extension}` : ''}{item.size ? ` · ${fmtBytes(item.size)}` : (probed ? ' · ?' : '')}</small>
                  </span>
                </label>
              ))}
            </div>
          </section>
        )}
      </>
    )}
    <div className="batch-options">
      <div className="batch-referer">
        <label htmlFor="batch-referer">Referer（可选）</label>
        <Input id="batch-referer" placeholder={REQUEST_EXAMPLES.referer} value={referer} onChange={event => setReferer(event.target.value)} />
        <small>{REQUEST_FIELD_HELP.referer}</small>
      </div>
      <label htmlFor="batch-concurrency">并发</label>
      <Input id="batch-concurrency" className="number-input" type="number" min={1} max={64} value={concurrency} onChange={event => setConcurrency(Math.max(1, Math.min(64, Number(event.target.value))))} />
      <Button className="primary-button" onClick={() => void submit()} disabled={loading || !addCount}>{loading ? '添加中...' : `添加 ${addCount} 项`}</Button>
    </div>
    {error && <div className="inline-error" role="alert">{error}</div>}
  </div>
}

