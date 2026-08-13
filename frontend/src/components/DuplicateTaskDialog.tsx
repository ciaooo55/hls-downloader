import { useEffect } from 'react'
import { AlertTriangle } from 'lucide-react'
import { Button, Dialog, DialogFooter, DialogOverlay } from './ui'
import { duplicateActionLabel, primaryDuplicate, type DuplicateMatch } from '../duplicateTask'

export default function DuplicateTaskDialog({
  message,
  duplicates,
  busy = false,
  onReuse,
  onAddNew,
  onCancel,
}: {
  message: string
  duplicates: DuplicateMatch[]
  busy?: boolean
  onReuse: (match: DuplicateMatch) => void
  onAddNew: () => void
  onCancel: () => void
}) {
  const top = primaryDuplicate(duplicates)
  const reuseLabel = duplicateActionLabel(top?.suggested_action)
  useEffect(() => {
    const close = (event: KeyboardEvent) => { if (event.key === 'Escape') onCancel() }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [onCancel])
  return (
    <DialogOverlay className="confirm-overlay" onClose={onCancel}>
      <Dialog className="confirm-modal" role="alertdialog" aria-labelledby="duplicate-title" aria-describedby="duplicate-message" label={"\u68c0\u6d4b\u5230\u91cd\u590d\u4e0b\u8f7d"}>
        <header>
          <div className="confirm-heading">
            <AlertTriangle size={20} />
            <div>
              <h2 id="duplicate-title">{"\u68c0\u6d4b\u5230\u91cd\u590d\u4e0b\u8f7d"}</h2>
              <p id="duplicate-message">{message}{top?.filename ? ` \u00b7 ${top.filename}` : ''}</p>
            </div>
          </div>
        </header>
        <DialogFooter>
          <Button variant="secondary" className="secondary-button" disabled={busy} onClick={onCancel}>{"\u53d6\u6d88"}</Button>
          <Button variant="secondary" className="secondary-button" disabled={busy} onClick={onAddNew}>{"\u4ecd\u6dfb\u52a0\u65b0\u4efb\u52a1"}</Button>
          {top && reuseLabel ? <Button className="primary-button" disabled={busy} onClick={() => onReuse(top)}>{reuseLabel}</Button> : null}
        </DialogFooter>
      </Dialog>
    </DialogOverlay>
  )
}
