import { useEffect, useState } from 'react'
import { Check, RefreshCw, ScreenShare, Tv, X } from 'lucide-react'
import { scanCastDevices, scanTvboxDevices } from '../api'
import { Button, Dialog, DialogOverlay } from './ui'

type Mode = 'cast' | 'tvbox'

export default function DevicePickerDialog({ mode, onChoose, onClose }: { mode: Mode; onChoose: (device: any) => void; onClose: () => void }) {
  const [devices, setDevices] = useState<any[]>([])
  const [selected, setSelected] = useState<any>(null)
  const [busy, setBusy] = useState(true)
  const [error, setError] = useState('')
  const scan = async () => {
    setBusy(true); setError(''); setSelected(null)
    try {
      const result = mode === 'cast' ? await scanCastDevices() : await scanTvboxDevices()
      setDevices(result.devices || [])
      if (!result.devices?.length) setError('未发现可用设备。请确认电脑与电视在同一局域网，并关闭 VPN/代理后重试。')
    } catch (reason: any) { setError(reason.message || '设备搜索失败') } finally { setBusy(false) }
  }
  useEffect(() => { void scan() }, [mode])
  const label = mode === 'cast' ? '投屏' : 'TVBox 推送'
  const deviceName = (device: any) => mode === 'cast' ? device.label : device.label || device.host
  const deviceDetail = (device: any) => mode === 'cast'
    ? `${device.protocol === 'chromecast' ? 'Chromecast' : 'DLNA'} · ${device.host}`
    : `${device.host}:${device.port}`
  const selectedName = selected ? deviceName(selected) : ''
  return <DialogOverlay onClose={onClose}><Dialog className="device-picker" label={`选择${label}设备`}>
    <header><div><h2>选择{label}设备</h2><p>{busy ? '正在搜索同一局域网内的可用设备…' : devices.length ? `已发现 ${devices.length} 台设备；发送前请确认目标。` : '每次发送前确认目标，避免投到错误的电视。'}</p></div><button className="modal-close-button" onClick={onClose}><X size={18} /></button></header>
    <div className="device-picker-list" aria-busy={busy}>{devices.map(device => {
      const name = deviceName(device)
      const detail = deviceDetail(device)
      const active = selected && (mode === 'cast' ? selected.id === device.id : selected.endpoint === device.endpoint)
      return <button key={mode === 'cast' ? device.id : device.endpoint} type="button" aria-pressed={Boolean(active)} className={active ? 'selected' : ''} onClick={() => setSelected(device)}><span>{mode === 'cast' ? <ScreenShare size={17} /> : <Tv size={17} />}</span><div><b>{name}</b><small>{detail}</small></div>{active && <Check className="device-picker-check" size={16} aria-hidden="true" />}</button>
    })}</div>
    {!busy && !devices.length && <div className="device-picker-empty"><span>{mode === 'cast' ? <ScreenShare size={20} /> : <Tv size={20} />}</span><b>暂未发现设备</b><small>确认电视已开启且与电脑连接同一局域网，然后重新搜索。</small></div>}
    {error && <p className="device-picker-message" role="status">{error}</p>}
    <footer><Button variant="secondary" className="secondary-button" disabled={busy} onClick={() => void scan()}><RefreshCw size={15} />{busy ? '正在搜索…' : '重新搜索'}</Button><Button disabled={!selected || busy} onClick={() => onChoose(selected)}>{selectedName ? `${label}到「${selectedName}」` : '请选择设备'}</Button></footer>
  </Dialog></DialogOverlay>
}
