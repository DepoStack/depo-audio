import * as React from 'react'
import { cn } from '../../lib/utils'

const Card = React.forwardRef(({ className, ...props }, ref) => (
  <div ref={ref} className={cn('rounded-lg border bg-card text-card-foreground overflow-hidden [box-shadow:var(--shadow-card)]', className)} {...props} />
))
Card.displayName = 'Card'

const CardHeader = React.forwardRef(({ className, ...props }, ref) => (
  <div ref={ref} className={cn('flex items-center justify-between px-4 py-2.5 border-b border-border/60', className)} {...props} />
))
CardHeader.displayName = 'CardHeader'

const CardTitle = React.forwardRef(({ className, ...props }, ref) => (
  <span ref={ref} className={cn('font-mono text-[9.5px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]', className)} {...props} />
))
CardTitle.displayName = 'CardTitle'

const CardContent = React.forwardRef(({ className, ...props }, ref) => (
  <div ref={ref} className={cn('', className)} {...props} />
))
CardContent.displayName = 'CardContent'

const CardFooter = React.forwardRef(({ className, ...props }, ref) => (
  <div ref={ref} className={cn('flex items-center px-4 py-2.5 border-t border-border/60', className)} {...props} />
))
CardFooter.displayName = 'CardFooter'

export { Card, CardHeader, CardTitle, CardContent, CardFooter }
