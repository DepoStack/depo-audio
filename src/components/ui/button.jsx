import * as React from 'react'
import { Slot } from '@radix-ui/react-slot'
import { cva } from 'class-variance-authority'
import { cn } from '../../lib/utils'

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-xs font-semibold transition-[color,background-color,border-color,box-shadow,transform] duration-150 ease-out active:translate-y-px focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:pointer-events-none disabled:translate-y-0 disabled:opacity-45',
  {
    variants: {
      variant: {
        default:
          'border border-border bg-[hsl(var(--surface))] text-[hsl(var(--text2))] hover:border-[hsl(var(--text2)/0.7)] hover:bg-secondary/70 hover:text-foreground',
        primary:
          'border border-primary bg-primary text-primary-foreground shadow-[inset_0_1px_0_hsl(var(--gold-hi)/0.45)] hover:bg-gold-hi',
        destructive: 'bg-destructive text-destructive-foreground hover:bg-destructive/90',
        outline: 'border border-border bg-transparent hover:bg-accent hover:text-accent-foreground',
        ghost: 'text-[hsl(var(--sub))] hover:text-foreground hover:bg-secondary/70',
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
  },
)

const Button = React.forwardRef(({ className, variant, size, asChild = false, ...props }, ref) => {
  const Comp = asChild ? Slot : 'button'
  return <Comp className={cn(buttonVariants({ variant, size, className }))} ref={ref} {...props} />
})
Button.displayName = 'Button'

export { Button }
