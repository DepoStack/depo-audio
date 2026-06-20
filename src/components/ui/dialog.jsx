import * as React from 'react'
import * as DialogPrimitive from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { cn } from '../../lib/utils'

const Dialog = DialogPrimitive.Root
const DialogTrigger = DialogPrimitive.Trigger
const DialogPortal = DialogPrimitive.Portal

// Close button with a visible icon and screen-reader name by default —
// a bare <DialogClose /> previously rendered an empty, invisible button.
// Usages with children or asChild are passed through untouched.
const DialogClose = React.forwardRef(({ className, children, asChild, ...props }, ref) => {
  if (asChild || children) {
    return (
      <DialogPrimitive.Close ref={ref} asChild={asChild} className={className} {...props}>
        {children}
      </DialogPrimitive.Close>
    )
  }
  return (
    <DialogPrimitive.Close
      ref={ref}
      className={cn(
        'rounded-md p-1.5 text-[hsl(var(--sub))] transition-colors hover:text-foreground hover:bg-secondary',
        'focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
        className
      )}
      {...props}
    >
      <X size={15} aria-hidden="true" />
      <span className="sr-only">Close</span>
    </DialogPrimitive.Close>
  )
})
DialogClose.displayName = 'DialogClose'

const DialogOverlay = React.forwardRef(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    className={cn('fixed inset-0 z-50 bg-black/60 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0', className)}
    {...props}
  />
))
DialogOverlay.displayName = 'DialogOverlay'

const DialogContent = React.forwardRef(({ className, children, ...props }, ref) => (
  <DialogPortal>
    <DialogOverlay />
    <DialogPrimitive.Content
      ref={ref}
      className={cn(
        'fixed left-[50%] top-[50%] z-50 w-full max-w-[480px] translate-x-[-50%] translate-y-[-50%]',
        'bg-card border border-border rounded-xl shadow-[0_20px_60px_rgba(0,0,0,0.5)] max-h-[90vh] overflow-y-auto',
        'data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95',
        className
      )}
      {...props}
    >
      {children}
    </DialogPrimitive.Content>
  </DialogPortal>
))
DialogContent.displayName = 'DialogContent'

const DialogHeader = ({ className, ...props }) => (
  <div className={cn('flex items-center justify-between px-5 pt-4 pb-3 border-b border-border/60', className)} {...props} />
)

const DialogTitle = React.forwardRef(({ className, ...props }, ref) => (
  <DialogPrimitive.Title ref={ref} className={cn('text-[15px] font-semibold text-foreground', className)} {...props} />
))
DialogTitle.displayName = 'DialogTitle'

const DialogDescription = React.forwardRef(({ className, ...props }, ref) => (
  <DialogPrimitive.Description ref={ref} className={cn('text-xs text-[hsl(var(--sub))] leading-relaxed', className)} {...props} />
))
DialogDescription.displayName = 'DialogDescription'

const DialogFooter = ({ className, ...props }) => (
  <div className={cn('flex justify-end gap-2 px-5 py-4 border-t border-border/60 mt-3.5', className)} {...props} />
)

export { Dialog, DialogPortal, DialogOverlay, DialogClose, DialogTrigger, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription }
