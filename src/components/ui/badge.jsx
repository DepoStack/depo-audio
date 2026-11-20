import * as React from 'react'
import { cva } from 'class-variance-authority'
import { cn } from '../../lib/utils'

const badgeVariants = cva(
  'inline-flex items-center type-label tabular-nums whitespace-nowrap rounded-sm px-1.5 py-0.5',
  {
    variants: {
      variant: {
        default: 'bg-secondary text-[hsl(var(--sub))]',
        active: 'bg-[hsl(var(--gold-dim))] text-foreground',
        done: 'bg-[hsl(var(--success)/0.1)] text-success',
        error: 'border border-destructive text-destructive',
        info: 'bg-[hsl(var(--blue)/0.1)] border border-[hsl(var(--blue)/0.18)] text-[hsl(var(--blue))]',
        warning: 'bg-[hsl(var(--warning)/0.1)] text-warning',
        tag: 'bg-[hsl(var(--blue)/0.1)] text-[hsl(var(--blue))]',
        outline: 'border border-border text-[hsl(var(--text2))]',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  },
)

const Badge = React.forwardRef(({ className, variant, ...props }, ref) => (
  <span ref={ref} className={cn(badgeVariants({ variant, className }))} {...props} />
))
Badge.displayName = 'Badge'

export { Badge }
