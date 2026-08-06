import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, LoaderCircle, LockKeyhole, ScrollText, ShieldCheck, X } from 'lucide-react'

import { acceptLegalTerms, fetchLegalTerms } from '../api'
import type { LegalDocument, LegalStatus } from '../types'
import { Button, Dialog, DialogOverlay } from './ui'


type LegalTab = 'terms' | 'privacy'

export default function LegalAgreementDialog({
  status,
  required,
  loadError = '',
  onRetry,
  onAccepted,
  onClose,
  onExit,
}: {
  status: LegalStatus | null
  required: boolean
  loadError?: string
  onRetry?: () => void
  onAccepted?: (status: LegalStatus) => void
  onClose?: () => void
  onExit?: () => void
}) {
  const [legalDocument, setLegalDocument] = useState<LegalDocument | null>(null)
  const [documentError, setDocumentError] = useState('')
  const [activeTab, setActiveTab] = useState<LegalTab>('terms')
  const [accepted, setAccepted] = useState(false)
  const [accepting, setAccepting] = useState(false)
  const dialogRef = useRef<HTMLElement>(null)
  const firstActionRef = useRef<HTMLButtonElement>(null)

  const loadDocument = () => {
    if (!status) return
    setLegalDocument(null)
    setDocumentError('')
    void fetchLegalTerms()
      .then(value => {
        if (value.document_digest !== status.document_digest || value.required_version !== status.required_version) {
          throw new Error('协议内容已经变化，请重新加载')
        }
        setLegalDocument(value)
      })
      .catch(reason => setDocumentError(reason?.message || '无法读取用户协议'))
  }

  useEffect(loadDocument, [status?.document_digest, status?.required_version])

  useEffect(() => {
    const timer = window.setTimeout(() => {
      if (firstActionRef.current) firstActionRef.current.focus()
      else dialogRef.current?.focus()
    }, 0)
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        if (!required) onClose?.()
        event.preventDefault()
        return
      }
      if (event.key !== 'Tab') return
      const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
      )
      if (!focusable?.length) return
      const items = Array.from(focusable)
      const first = items[0]
      const last = items[items.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', keydown)
    return () => {
      window.clearTimeout(timer)
      window.removeEventListener('keydown', keydown)
    }
  }, [required, onClose])

  const accept = async () => {
    if (!legalDocument || !accepted || accepting) return
    setAccepting(true)
    setDocumentError('')
    try {
      const next = await acceptLegalTerms({
        version: legalDocument.required_version,
        document_digest: legalDocument.document_digest,
        accepted,
      })
      onAccepted?.(next)
    } catch (reason: any) {
      setDocumentError(reason?.message || '无法保存协议接受记录')
      setAccepting(false)
    }
  }

  const acceptedAt = status?.accepted_at
    ? new Date(status.accepted_at).toLocaleString('zh-CN', { hour12: false })
    : ''

  return <DialogOverlay className="legal-overlay" onClose={required ? undefined : onClose}>
    <Dialog
      ref={dialogRef}
      className="legal-dialog"
      role={required ? 'alertdialog' : 'dialog'}
      aria-labelledby="legal-dialog-title"
      aria-describedby="legal-dialog-description"
      tabIndex={-1}
    >
      <header className="legal-header">
        <div className="legal-title-mark" aria-hidden="true"><ShieldCheck size={22} /></div>
        <div>
          <h2 id="legal-dialog-title">{required ? '使用前请确认' : '用户协议与隐私政策'}</h2>
          <p id="legal-dialog-description">中国大陆版 · 协议与隐私政策仅在本机确认</p>
        </div>
        {!required && <Button ref={firstActionRef} variant="ghost" size="icon" aria-label="关闭" title="关闭" onClick={onClose}><X size={18} /></Button>}
      </header>

      {!status && <div className="legal-loading" role="status">
        {loadError ? <>
          <AlertTriangle size={24} />
          <strong>无法核对协议状态</strong>
          <span>{loadError}</span>
          <div><Button ref={firstActionRef} variant="secondary" onClick={onRetry}>重新加载</Button>{required && <Button variant="ghost" onClick={onExit}>退出软件</Button>}</div>
        </> : <><LoaderCircle className="spin" size={24} /><span>正在读取本机协议状态…</span></>}
      </div>}

      {status && <>
        <div className="legal-document-shell">
          <div className="legal-tabs" role="tablist" aria-label="法律文档">
            <button type="button" role="tab" aria-selected={activeTab === 'terms'} onClick={() => setActiveTab('terms')}><ScrollText size={15} />用户协议与免责声明</button>
            <button type="button" role="tab" aria-selected={activeTab === 'privacy'} onClick={() => setActiveTab('privacy')}><LockKeyhole size={15} />隐私政策</button>
          </div>
          {documentError && <div className="legal-inline-error" role="alert"><span>{documentError}</span><Button variant="secondary" size="sm" onClick={loadDocument}>重试</Button></div>}
          {!legalDocument && !documentError && <div className="legal-document-loading" role="status"><LoaderCircle className="spin" size={18} />正在读取完整文档…</div>}
          {legalDocument && <pre className="legal-document" tabIndex={0}>{activeTab === 'terms' ? legalDocument.content : legalDocument.privacy_content}</pre>}
        </div>

        <div className="legal-record-note">
          <span>版本 {status.required_version}</span>
          <span>摘要 {status.document_digest.slice(0, 12)}…</span>
          {status.accepted && acceptedAt && <span>本机已于 {acceptedAt} 接受</span>}
        </div>

        {required && <label className="legal-single-confirmation">
          <input type="checkbox" checked={accepted} onChange={event => setAccepted(event.target.checked)} />
          <span>我已阅读并同意《用户协议与免责声明》和《隐私政策》，并仅使用本软件处理我有权处理的内容。</span>
        </label>}

        <footer className="legal-footer">
          <span>{required ? '接受记录只保存版本、摘要和时间，不会上传。' : '删除或重置本机配置后会再次询问。'}</span>
          <div>
            {required
              ? <><Button ref={firstActionRef} variant="secondary" onClick={onExit}>不同意并退出</Button><Button disabled={!legalDocument || !accepted || accepting} onClick={() => void accept()}>{accepting ? '正在保存…' : '同意并继续'}</Button></>
              : <Button onClick={onClose}>关闭</Button>}
          </div>
        </footer>
      </>}
    </Dialog>
  </DialogOverlay>
}
