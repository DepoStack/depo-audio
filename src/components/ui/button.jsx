import * as React from 'react'
import { Slot } from '@radix-ui/react-slot'
import { cva } from 'class-variance-authority'
import { cn } from '../../lib/utils'

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-xs font-semibold transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-45',
  {
    variants: {
      variant: {
        default: 'border border-border bg-card text-[hsl(var(--text2))] hover:border-[hsl(var(--text2))] hover:text-foreground',
        primary: 'bg-primary text-primary-foreground shadow-[0_3px_16px_hsl(var(--gold-glow))] hover:bg-gold-hi hover:shadow-[0_3px_24px_hsl(var(--gold-glow))] hover:-translate-y-px',
        destructive: 'bg-destructive text-destructive-foreground hover:bg-destructive/90',
        outline: 'border border-border bg-transparent hover:bg-accent hover:text-accent-foreground',
        ghost: 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))] hover:bg-card',
        scan: 'font-mono text-[10px] tracking-wider uppercase border border-[hsl(var(--blue))] text-[hsl(var(--blue))] bg-[hsl(var(--blue)/0.08)] hover:bg-[hsl(var(--blue)/0.18)] disabled:opacity-40',
      },
      size: {
        default: 'h-8 px-3.5 py-1.5',
        sm: 'h-7 px-2.5 text-[11px]',
        lg: 'h-10 px-6 text-[13px]',
        icon: 'h-8 w-8',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  }
)

const Button = React.forwardRef(({ className, variant, size, asChild = false, ...props }, ref) => {
  const Comp = asChild ? Slot : 'button'
  return (
    <Comp className={cn(buttonVariants({ variant, size, className }))} ref={ref} {...props} />
  )
})
Button.displayName = 'Button'

export { Button, buttonVariants }
