import { useEffect, useState } from 'react'
import { Download, ExternalLink, FolderOpen, Puzzle, X } from 'lucide-react'
import { fetchBrowserStatus, fetchUserscriptStatus } from '../api'
import { exportUserscript, openBrowserExtensionInstaller, openUserscriptInstaller } from '../desktop'
import type { BrowserStatus, UserscriptStatus } from '../types'

export default function UserscriptDialog({ onClose }: { onClose: () => void }) {
  const [status, setStatus] = useState<UserscriptStatus | null>(null)
  const [browserStatus, setBrowserStatus] = useState<BrowserStatus | null>(null)
  const [message, setMessage] = useState('')
  useEffect(() => {
    fetchUserscriptStatus().then(setStatus).catch(() => setStatus(null))
    fetchBrowserStatus().then(setBrowserStatus).catch(() => setBrowserStatus(null))
  }, [])
  const install = async () => { const result = await openUserscriptInstaller(); setMessage(result.ok ? '已打开浏览器，请在 ScriptCat 或 Tampermonkey 中确认安装。' : result.error || '无法打开安装地址') }
  const exportFile = async () => { const result = await exportUserscript(); setMessage(result.ok ? `已导出到 ${result.path}` : result.canceled ? '已取消导出' : result.error || '导出失败') }
  const installExtension = async () => { const result = await openBrowserExtensionInstaller(); setMessage(result.ok ? `${result.browser_opened ? '已打开 Chrome 扩展页和扩展目录。' : '已打开扩展目录。请手动打开 Chrome 的 chrome://extensions。'} 开启“开发者模式”，点击“加载已解压的扩展程序”，选择：${result.path}` : result.error || '无法打开扩展安装工具') }
  return <div className="modal-overlay" onMouseDown={onClose}><section className="modal userscript-modal" onMouseDown={event => event.stopPropagation()}>
    <header><div><h2>浏览器集成</h2><p>正式扩展负责接管下载，ScriptCat/Tampermonkey 脚本用于后备媒体嗅探</p></div><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <div className={`userscript-status ${browserStatus?.detected ? 'online' : ''}`}><Puzzle size={18} /><div><strong>{browserStatus?.detected ? '正式扩展已连接' : browserStatus?.seen_before ? '正式扩展连接已断开' : '正式扩展未安装或未连接'}</strong><span>{browserStatus?.detected ? `版本 ${browserStatus.version || '未知'}` : '没有正式扩展时，浏览器会继续使用自己的下载器。'}</span></div></div>
    <div className="script-actions"><button className="primary-button" onClick={installExtension}><FolderOpen size={16} />加载 Chromium 扩展</button></div>
    <div className={`userscript-status ${status?.detected ? 'online' : ''}`}><i /><div><strong>{status?.detected ? '已检测到脚本运行' : status?.seen_before ? '脚本此前运行过' : '本次尚未检测到脚本'}</strong><span>{status?.detected ? `${status.version || '未知版本'} · ${status.page_origin || '未知页面'}` : '安装后打开一个 HTTPS 视频页面，状态会自动更新。'}</span></div></div>
    <div className="script-actions"><button className="primary-button" onClick={install}><ExternalLink size={16} />直接安装脚本</button><button className="secondary-button" onClick={exportFile}><Download size={16} />导出到指定目录</button></div>
    {message && <div className="inline-message">{message}</div>}
    <p className="fine-print">新版脚本默认折叠为右上角小按钮，可展开、换边并记住位置；支持批量发送、手动添加、暂停、继续、取消、重试和打开成品。下载请求自动使用当前网页的 Referer、Origin、User-Agent 和 Cookie。</p>
  </section></div>
}
