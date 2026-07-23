import { cva, type VariantProps } from 'class-variance-authority'
import type { HTMLAttributes } from 'react'
import { cn } from '@/lib/cn'

const badgeVariants = cva(
  'status inline-flex items-center gap-1 rounded-[6px] px-1.5 py-0.5 text-[11px] font-semibold',
  {
    variants: {
      tone: {
        neutral: 'text-[var(--muted)]',
        info: 'status-downloading text-[var(--primary)]',
        success: 'status-done text-[var(--green)]',
        warning: 'status-paused text-[var(--amber)]',
        danger: 'status-failed text-[var(--red)]',
        accent: 'status-remuxing text-[var(--purple)]',
      },
    },
    defaultVariants: { tone: 'neutral' },
  },
)

export function statusTone(status: string): NonNullable<VariantProps<typeof badgeVariants>['tone']> {
  if (['done'].includes(status)) return 'success'
  if (['failed', 'unsupported', 'canceled'].includes(status)) return 'danger'
  if (['paused', 'pausing', 'merging', 'queued'].includes(status)) return 'warning'
  if (['remuxing', 'parsing', 'fetching_metadata', 'checking'].includes(status)) return 'accent'
  if (status.startsWith('download') || status === 'awaiting_confirmation') return 'info'
  return 'neutral'
}

export function Badge({
  className,
  tone,
  ...props
}: HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />
}
