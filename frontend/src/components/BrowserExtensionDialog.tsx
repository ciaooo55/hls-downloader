import { useEffect, useState } from 'react'
import { FolderOpen, Puzzle } from 'lucide-react'
import { fetchBrowserStatus } from '../api'
import { openBrowserExtensionInstaller } from '../desktop'
import type { BrowserStatus } from '../types'
import { Button, Dialog, DialogFooter, DialogHeader, DialogOverlay } from './ui'

export default function BrowserExtensionDialog({ onClose }: { onClose: () => void }) {
  const [browserStatus, setBrowserStatus] = useState<BrowserStatus | null>(null)
  const [message, setMessage] = useState('')
  const versionLine = browserStatus?.detected
    ? `${browserStatus.client_count || 1} 个浏览器插件 · 桌面 v${browserStatus.desktop_version || '未知'}`
    : '未连接时浏览器会继续使用自己的下载器，不会静默丢失文件。'
  const browserNames = { edge: 'Edge', chrome: 'Chrome', chromium: 'Chromium', firefox: 'Firefox', unknown: '浏览器' }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKeyDown)
    fetchBrowserStatus().then(setBrowserStatus).catch(() => setBrowserStatus(null))
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  const installExtension = async () => {
    const result = await openBrowserExtensionInstaller()
    setMessage(result.ok
      ? `${result.browser_opened ? '已打开 Chrome 扩展页和插件目录。' : '已打开插件目录，请手动打开 chrome://extensions。'} 开启“开发者模式”，点击“加载已解压的扩展程序”，选择：${result.path}`
      : result.error || '无法打开插件安装工具')
  }

  return (
    <DialogOverlay onClose={onClose}>
      <Dialog className="browser-integration-modal" label="浏览器插件" onClose={onClose}>
        <DialogHeader title="浏览器插件" description="Chrome/Edge 与 Firefox 插件负责资源识别、下载点击接管和请求身份传递" onClose={onClose} />
        <div className={`browser-status ${browserStatus?.detected ? 'online' : ''}${browserStatus?.needs_upgrade ? ' warning' : ''}`}>
          <Puzzle size={18} />
          <div>
            <strong>{browserStatus?.needs_upgrade ? '浏览器插件需要升级' : browserStatus?.detected ? '浏览器插件已连接' : browserStatus?.seen_before ? '插件连接已断开' : '插件未安装或未连接'}</strong>
            <span>{versionLine}</span>
          </div>
        </div>
        {!!browserStatus?.clients?.length && (
          <div className="browser-client-list" aria-label="浏览器插件连接">
            {browserStatus.clients.map(client => (
              <div className={!client.active ? 'inactive' : client.needs_upgrade ? 'warning' : 'online'} key={client.id}>
                <span className="browser-client-dot" />
                <strong>{browserNames[client.browser] || '浏览器'}</strong>
                <span>v{client.version || '未知'}</span>
                <em>{!client.active ? '未连接' : client.needs_upgrade ? '需升级' : '已连接'}</em>
              </div>
            ))}
          </div>
        )}
        {browserStatus?.needs_upgrade && (
          <div className="inline-message update-warning" role="status">
            当前插件低于推荐版本 v{browserStatus.recommended_version || '最新'}。请使用同一个 GitHub Release 里的 Chromium / Firefox 插件包；旧插件不会被强制禁用，但可能缺少最新的请求头、Cookie 和弹窗修复。
          </div>
        )}
        <div className="extension-actions">
          <Button className="primary-button" onClick={() => void installExtension()}><FolderOpen size={16} />加载 Chromium 插件</Button>
        </div>
        <section className="firefox-release-variants" aria-labelledby="firefox-release-variants-title">
          <div className="firefox-release-variants-heading">
            <strong id="firefox-release-variants-title">Firefox 发布包</strong>
            <span>功能完全相同，仅发布入口与扩展 ID 不同</span>
          </div>
          <div className="firefox-release-variant">
            <div><b>网页显示版（Firefox 商店）</b><small>用于已发布的 AMO 条目</small></div>
            <code>browser@hls-downloader.ciaooo55.com</code>
          </div>
          <div className="firefox-release-variant">
            <div><b>网页不显示版（独立包）</b><small>用于 GitHub Release 独立发布</small></div>
            <code>hls-downloader-store@ciaooo55.com</code>
          </div>
        </section>
        {message && <div className="inline-message">{message}</div>}
        <p className="fine-print">安装包内置 Chromium 插件目录。Firefox 请从同版本 GitHub Release 选择对应变体，并使用 Mozilla 签名版长期安装；不要把一个 ID 的更新上传到另一个 ID 的商店条目。Cookie 只在你对站点明确授权后读取。</p>
        <DialogFooter>
          <Button variant="secondary" className="secondary-button" onClick={onClose}>关闭</Button>
        </DialogFooter>
      </Dialog>
    </DialogOverlay>
  )
}
