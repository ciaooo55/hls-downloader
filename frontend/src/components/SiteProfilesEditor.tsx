import { useState } from 'react'
import { ChevronDown, ChevronUp, Plus, Trash2 } from 'lucide-react'
import { pickFolder } from '../desktop'
import {
  emptySiteProfile,
  headersToLines,
  linesToHeaders,
  moveSiteProfile,
  normalizeSiteProfiles,
  SITE_PROFILE_LIMIT,
  type SiteProfile,
} from '../siteProfiles'

const LABEL = {
  add: '\u6dfb\u52a0\u89c4\u5219',
  host: '\u4e3b\u673a\u540d / \u901a\u914d\u7b26',
  enabled: '\u542f\u7528',
  dir: '\u4fdd\u5b58\u5230\u6b64\u76ee\u5f55',
  dirHint: '\u7559\u7a7a\u5219\u7528\u5168\u5c40\u76ee\u5f55\uff1b\u6d4f\u89c8\u5668\u9009\u62e9\u7684\u4efb\u52a1\u76ee\u5f55\u4f18\u5148',
  cookie: 'Cookie',
  cookieHint: '\u6d4f\u89c8\u5668\u5b9e\u9645\u6355\u83b7\u7684 Cookie \u4f18\u5148',
  referer: 'Referer',
  origin: 'Origin',
  ua: 'User-Agent',
  concurrency: '\u5e76\u53d1\uff080=\u9ed8\u8ba4\uff09',
  speed: '\u9650\u901f KiB/s\uff080=\u4e0d\u9650\uff09',
  proxy: '\u4ee3\u7406',
  proxyInherit: '\u8ddf\u968f\u5168\u5c40',
  proxyDirect: '\u76f4\u8fde',
  proxySystem: '\u7cfb\u7edf\u4ee3\u7406',
  proxyManual: '\u6307\u5b9a\u4ee3\u7406',
  proxyUrl: '\u4ee3\u7406\u5730\u5740',
  proxyHint: '\u7559\u7a7a\u5219\u8ddf\u968f\u5168\u5c40\u4ee3\u7406\uff1b\u4ec5\u5bf9\u6b64\u4e3b\u673a\u751f\u6548',
  headers: '\u989d\u5916\u8bf7\u6c42\u5934\uff08\u6bcf\u884c Name: value\uff09',
  up: '\u4e0a\u79fb',
  down: '\u4e0b\u79fb',
  remove: '\u5220\u9664',
  pick: '\u9009\u62e9\u76ee\u5f55',
  json: '\u9ad8\u7ea7 JSON',
  first: '\u4ece\u4e0a\u5230\u4e0b\u7b2c\u4e00\u6761\u5339\u914d\u89c4\u5219\u751f\u6548\u3002\u7a7a\u89c4\u5219\u4e0d\u6539\u53d8\u4efb\u4f55\u9ed8\u8ba4\u4e0b\u8f7d\u884c\u4e3a\u3002\u4ee3\u7406\u7559\u7a7a\u5219\u7ee7\u7eed\u7528\u5168\u5c40\u8bbe\u7f6e\u3002',
}

export default function SiteProfilesEditor({
  value,
  onChange,
}: {
  value?: SiteProfile[]
  onChange: (profiles: SiteProfile[]) => void
}) {
  const profiles = Array.isArray(value) ? value : []
  const [showJson, setShowJson] = useState(false)
  const [jsonText, setJsonText] = useState('')
  const [jsonError, setJsonError] = useState('')

  const update = (index: number, patch: Partial<SiteProfile>) => {
    onChange(profiles.map((item, current) => current === index ? { ...item, ...patch } : item))
  }

  const openJson = () => {
    setJsonText(JSON.stringify(profiles, null, 2))
    setJsonError('')
    setShowJson(true)
  }

  const applyJson = () => {
    try {
      const parsed = JSON.parse(jsonText || '[]')
      if (!Array.isArray(parsed)) throw new Error()
      onChange(normalizeSiteProfiles(parsed))
      setShowJson(false)
      setJsonError('')
    } catch {
      setJsonError('\u5fc5\u987b\u662f JSON \u6570\u7ec4')
    }
  }

  return (
    <div className="site-profiles">
      <p className="field-note">{LABEL.first}</p>
      {profiles.map((profile, index) => (
        <section key={`${profile.host}-${index}`} className="site-profile-card">
          <header>
            <label className="checkbox-label">
              <input type="checkbox" checked={profile.enabled !== false} onChange={(event) => update(index, { enabled: event.target.checked })} />
              {LABEL.enabled}
            </label>
            <div className="site-profile-actions">
              <button type="button" className="icon-button bordered" title={LABEL.up} disabled={index === 0} onClick={() => onChange(moveSiteProfile(profiles, index, index - 1))}><ChevronUp size={16} /></button>
              <button type="button" className="icon-button bordered" title={LABEL.down} disabled={index === profiles.length - 1} onClick={() => onChange(moveSiteProfile(profiles, index, index + 1))}><ChevronDown size={16} /></button>
              <button type="button" className="icon-button bordered" title={LABEL.remove} onClick={() => onChange(profiles.filter((_, current) => current !== index))}><Trash2 size={16} /></button>
            </div>
          </header>
          <div className="settings-field">
            <label>{LABEL.host}</label>
            <input value={profile.host} onChange={(event) => update(index, { host: event.target.value })} placeholder="*.example.com" />
          </div>
          <div className="settings-field">
            <label>{LABEL.dir}</label>
            <div className="input-action">
              <input value={profile.download_dir || ''} onChange={(event) => update(index, { download_dir: event.target.value })} placeholder={LABEL.dirHint} />
              <button type="button" className="secondary-button" onClick={() => void (async () => {
                const native = await pickFolder(profile.download_dir || '')
                if (native.ok && native.path) update(index, { download_dir: native.path })
              })()}>{LABEL.pick}</button>
            </div>
          </div>
          <div className="settings-field">
            <label>{LABEL.cookie}</label>
            <input value={profile.cookie || ''} onChange={(event) => update(index, { cookie: event.target.value })} placeholder={LABEL.cookieHint} autoComplete="off" />
          </div>
          <div className="site-profile-grid">
            <div className="settings-field"><label>{LABEL.referer}</label><input value={profile.referer || ''} onChange={(event) => update(index, { referer: event.target.value })} /></div>
            <div className="settings-field"><label>{LABEL.origin}</label><input value={profile.origin || ''} onChange={(event) => update(index, { origin: event.target.value })} /></div>
            <div className="settings-field settings-field-wide"><label>{LABEL.ua}</label><input value={profile.user_agent || ''} onChange={(event) => update(index, { user_agent: event.target.value })} /></div>
            <div className="settings-field"><label>{LABEL.concurrency}</label><input type="number" min={0} max={64} value={profile.concurrency ?? 0} onChange={(event) => update(index, { concurrency: Number(event.target.value) || 0 })} /></div>
            <div className="settings-field"><label>{LABEL.speed}</label><input type="number" min={0} max={1048576} value={profile.speed_limit_kib ?? 0} onChange={(event) => update(index, { speed_limit_kib: Number(event.target.value) || 0 })} /></div>
            <div className="settings-field"><label>{LABEL.proxy}</label>
              <select value={profile.proxy_mode || ''} onChange={(event) => update(index, { proxy_mode: event.target.value as SiteProfile['proxy_mode'], proxy_url: event.target.value === 'manual' ? (profile.proxy_url || '') : '' })}>
                <option value="">{LABEL.proxyInherit}</option>
                <option value="direct">{LABEL.proxyDirect}</option>
                <option value="system">{LABEL.proxySystem}</option>
                <option value="manual">{LABEL.proxyManual}</option>
              </select>
            </div>
            {profile.proxy_mode === 'manual' ? <div className="settings-field settings-field-wide"><label>{LABEL.proxyUrl}</label><input value={profile.proxy_url || ''} onChange={(event) => update(index, { proxy_url: event.target.value })} placeholder="socks5://127.0.0.1:1080" autoComplete="off" /></div> : null}
          </div>
          <div className="settings-field">
            <label>{LABEL.headers}</label>
            <textarea rows={3} value={headersToLines(profile.request_headers)} onChange={(event) => update(index, { request_headers: linesToHeaders(event.target.value) })} spellCheck={false} />
          </div>
        </section>
      ))}
      <div className="site-profile-toolbar">
        <button type="button" className="secondary-button" disabled={profiles.length >= SITE_PROFILE_LIMIT} onClick={() => onChange([...profiles, emptySiteProfile()])}><Plus size={16} />{LABEL.add}</button>
        <button type="button" className="text-button" onClick={openJson}>{LABEL.json}</button>
      </div>
      {showJson && <div className="settings-field settings-field-wide">
        <textarea className="settings-json-editor" rows={8} value={jsonText} onChange={(event) => setJsonText(event.target.value)} spellCheck={false} />
        {jsonError && <p className="field-note">{jsonError}</p>}
        <button type="button" className="secondary-button" onClick={applyJson}>JSON</button>
      </div>}
    </div>
  )
}
