import { cva, type VariantProps } from 'class-variance-authority'
import type { ButtonHTMLAttributes } from 'react'
import { cn } from '@/lib/cn'

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-[8px] text-[12px] font-medium transition-[background-color,border-color,color,opacity,box-shadow] duration-150 ease-out cursor-pointer disabled:pointer-events-none disabled:opacity-40 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-[var(--primary)]',
  {
    variants: {
      variant: {
        default: 'h-[34px] px-3 border border-transparent bg-[var(--primary)] text-white hover:bg-[var(--primary-hover)]',
        secondary: 'h-[34px] px-3 border border-[var(--border)] bg-[var(--surface-2)] text-[var(--text)] hover:bg-[var(--surface-3)]',
        ghost: 'h-[34px] px-2 border border-transparent bg-transparent text-[var(--muted)] hover:bg-[var(--surface-3)] hover:text-[var(--text)]',
        danger: 'h-[34px] px-3 border border-[color-mix(in_srgb,var(--red)_55%,var(--border))] bg-[color-mix(in_srgb,var(--red)_12%,var(--surface))] text-[var(--red)] hover:bg-[color-mix(in_srgb,var(--red)_18%,var(--surface))]',
        tool: 'h-[34px] min-w-[34px] px-2 border border-transparent bg-transparent text-[var(--muted)] hover:bg-[var(--surface-3)] hover:text-[var(--text)]',
        primaryTool: 'h-[34px] min-w-[34px] px-3 border border-transparent text-white bg-[linear-gradient(135deg,var(--primary),#0f766e)] shadow-[0_3px_8px_color-mix(in_srgb,var(--primary)_28%,transparent)] hover:brightness-[1.04]',
      },
      size: {
        default: '',
        icon: 'px-0 w-[34px]',
        sm: 'h-7 px-2 text-[11px]',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
)

export interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

export function Button({ className, variant, size, type = 'button', ...props }: ButtonProps) {
  return <button type={type} className={cn(buttonVariants({ variant, size }), className)} {...props} />
}

export { buttonVariants }
