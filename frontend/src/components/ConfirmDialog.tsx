import { useEffect } from 'react'
import { AlertTriangle, X } from 'lucide-react'

export default function ConfirmDialog({ title, message, confirmLabel = '确定', danger = false, onConfirm, onCancel }: {
  title: string
  message: string
  confirmLabel?: string
  danger?: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  useEffect(() => {
    const close = (event: KeyboardEvent) => { if (event.key === 'Escape') onCancel() }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [onCancel])

  return <div className="modal-overlay confirm-overlay" onMouseDown={onCancel}>
    <section className="modal confirm-modal" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" aria-describedby="confirm-message" onMouseDown={event => event.stopPropagation()}>
      <header><div className="confirm-heading"><AlertTriangle size={20} /><div><h2 id="confirm-title">{title}</h2><p id="confirm-message">{message}</p></div></div><button className="icon-button" title="关闭" onClick={onCancel}><X size={17} /></button></header>
      <footer><button className="secondary-button" autoFocus onClick={onCancel}>取消</button><button className={danger ? 'danger-button' : 'primary-button'} onClick={onConfirm}>{confirmLabel}</button></footer>
    </section>
  </div>
}
