import type { HTMLAttributes, ReactNode } from 'react'
import { X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { Button } from './button'

export function DialogOverlay({
  className,
  onClose,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement> & { onClose?: () => void }) {
  return (
    <div
      className={cn('modal-overlay', className)}
      onMouseDown={onClose}
      {...props}
    >
      {children}
    </div>
  )
}

export function Dialog({
  className,
  children,
  onClose,
  label,
  ...props
}: HTMLAttributes<HTMLElement> & { onClose?: () => void; label?: string }) {
  return (
    <section
      className={cn('modal', className)}
      role="dialog"
      aria-modal="true"
      aria-label={label}
      onMouseDown={event => event.stopPropagation()}
      {...props}
    >
      {children}
    </section>
  )
}

export function DialogHeader({
  title,
  description,
  onClose,
  children,
}: {
  title: string
  description?: string
  onClose?: () => void
  children?: ReactNode
}) {
  return (
    <header>
      <div>
        <h2>{title}</h2>
        {description ? <p>{description}</p> : null}
      </div>
      <div className="header-actions">
        {children}
        {onClose ? (
          <Button variant="ghost" size="icon" className="icon-button" title="关闭" aria-label="关闭" onClick={onClose}>
            <X size={18} />
          </Button>
        ) : null}
      </div>
    </header>
  )
}

export function DialogFooter({ className, children }: { className?: string; children: ReactNode }) {
  return <footer className={className}>{children}</footer>
}
