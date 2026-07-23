import { useEffect } from 'react'
import { AlertTriangle } from 'lucide-react'
import { Button, Dialog, DialogFooter, DialogOverlay } from './ui'

export default function ConfirmDialog({
  title,
  message,
  confirmLabel = '确定',
  danger = false,
  onConfirm,
  onCancel,
}: {
  title: string
  message: string
  confirmLabel?: string
  danger?: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  useEffect(() => {
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCancel()
    }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [onCancel])

  return (
    <DialogOverlay className="confirm-overlay" onClose={onCancel}>
      <Dialog className="confirm-modal" role="alertdialog" aria-labelledby="confirm-title" aria-describedby="confirm-message" label={title}>
        <header>
          <div className="confirm-heading">
            <AlertTriangle size={20} />
            <div>
              <h2 id="confirm-title">{title}</h2>
              <p id="confirm-message">{message}</p>
            </div>
          </div>
        </header>
        <DialogFooter>
          <Button variant="secondary" className="secondary-button" autoFocus onClick={onCancel}>
            取消
          </Button>
          <Button variant={danger ? 'danger' : 'default'} className={danger ? 'danger-button' : 'primary-button'} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </DialogFooter>
      </Dialog>
    </DialogOverlay>
  )
}
