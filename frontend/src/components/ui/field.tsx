import type { ReactNode } from 'react'
import { cn } from '@/lib/cn'

export function Field({
  label,
  htmlFor,
  help,
  className,
  children,
}: {
  label: string
  htmlFor?: string
  help?: string
  className?: string
  children: ReactNode
}) {
  return (
    <div className={cn('settings-field', className)}>
      <label htmlFor={htmlFor}>{label}</label>
      {children}
      {help ? <p>{help}</p> : null}
    </div>
  )
}
