import { useEffect, useState } from 'react'
import { Download, ExternalLink, X } from 'lucide-react'
import { fetchUserscriptStatus } from '../api'
import { exportUserscript, openUserscriptInstaller } from '../desktop'
import type { UserscriptStatus } from '../types'

export default function UserscriptDialog({ onClose }: { onClose: () => void }) {
  const [status, setStatus] = useState<UserscriptStatus | null>(null)
  const [message, setMessage] = useState('')
  useEffect(() => { fetchUserscriptStatus().then(setStatus).catch(() => setStatus(null)) }, [])
  const install = async () => { const result = await openUserscriptInstaller(); setMessage(result.ok ? '已打开浏览器，请在 ScriptCat 或 Tampermonkey 中确认安装。' : result.error || '无法打开安装地址') }
  const exportFile = async () => { const result = await exportUserscript(); setMessage(result.ok ? `已导出到 ${result.path}` : result.canceled ? '已取消导出' : result.error || '导出失败') }
  return <div className="modal-overlay" onMouseDown={onClose}><section className="modal userscript-modal" onMouseDown={event => event.stopPropagation()}>
    <header><div><h2>浏览器脚本工具</h2><p>支持 ScriptCat 和 Tampermonkey，用于嗅探动态加载的 m3u8</p></div><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <div className={`userscript-status ${status?.detected ? 'online' : ''}`}><i /><div><strong>{status?.detected ? '已检测到脚本运行' : status?.seen_before ? '脚本此前运行过' : '本次尚未检测到脚本'}</strong><span>{status?.detected ? `${status.version || '未知版本'} · ${status.page_origin || '未知页面'}` : '安装后打开一个 HTTPS 视频页面，状态会自动更新。'}</span></div></div>
    <div className="script-actions"><button className="primary-button" onClick={install}><ExternalLink size={16} />直接安装脚本</button><button className="secondary-button" onClick={exportFile}><Download size={16} />导出到指定目录</button></div>
    {message && <div className="inline-message">{message}</div>}
    <p className="fine-print">新版脚本默认折叠为右上角小按钮，可展开、换边并记住位置；支持批量发送、手动添加、暂停、继续、取消、重试和打开成品。下载请求自动使用当前网页的 Referer、Origin、User-Agent 和 Cookie。</p>
  </section></div>
}
