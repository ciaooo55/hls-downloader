import { useEffect, useState } from 'react'
import { FolderOpen, Puzzle } from 'lucide-react'
import { fetchBrowserStatus } from '../api'
import { openBrowserExtensionInstaller } from '../desktop'
import type { BrowserStatus } from '../types'
import { Button, Dialog, DialogFooter, DialogHeader, DialogOverlay } from './ui'

export default function BrowserExtensionDialog({ onClose }: { onClose: () => void }) {
  const [browserStatus, setBrowserStatus] = useState<BrowserStatus | null>(null)
  const [message, setMessage] = useState('')

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
        <div className={`browser-status ${browserStatus?.detected ? 'online' : ''}`}>
          <Puzzle size={18} />
          <div>
            <strong>{browserStatus?.detected ? '浏览器插件已连接' : browserStatus?.seen_before ? '插件连接已断开' : '插件未安装或未连接'}</strong>
            <span>{browserStatus?.detected ? `版本 ${browserStatus.version || '未知'}` : '未连接时浏览器会继续使用自己的下载器，不会静默丢失文件。'}</span>
          </div>
        </div>
        <div className="extension-actions">
          <Button className="primary-button" onClick={() => void installExtension()}><FolderOpen size={16} />加载 Chromium 插件</Button>
        </div>
        {message && <div className="inline-message">{message}</div>}
        <p className="fine-print">安装包内置 Chromium 插件目录。Firefox 请从同版本 GitHub Release 下载插件包，并使用 Mozilla 签名版长期安装。Cookie 只在你对站点明确授权后读取。</p>
        <DialogFooter>
          <Button variant="secondary" className="secondary-button" onClick={onClose}>关闭</Button>
        </DialogFooter>
      </Dialog>
    </DialogOverlay>
  )
}
