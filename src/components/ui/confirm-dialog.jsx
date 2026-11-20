import { useRef } from 'react'
import { Dialog, DialogContent, DialogTitle, DialogDescription } from './dialog'
import { Button } from './button'

// Confirm dialog in the app's own Dialog grammar — replaces native
// window.confirm() for destructive actions so they don't break out of the
// DepoStack visual world (and gives keyboard + screen-reader semantics).
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel = 'Delete',
  cancelLabel = 'Cancel',
  destructive = true,
  onConfirm,
  // Optional middle choice, e.g. "Add to queue" next to "Replace"
  secondaryLabel,
  onSecondary,
  // Which button has focus when the dialog opens, and so which one Enter
  // presses: 'confirm' (default), 'secondary' or 'cancel'. Point it away from
  // a destructive confirm when a safer choice exists.
  initialFocus = 'confirm',
}) {
  const cancelRef = useRef(null)
  const secondaryRef = useRef(null)
  const confirmRef = useRef(null)
  const hasSecondary = !!(secondaryLabel && onSecondary)

  const focusInitial = event => {
    const target =
      initialFocus === 'cancel'
        ? cancelRef.current
        : initialFocus === 'secondary' && hasSecondary
          ? secondaryRef.current
          : confirmRef.current
    if (!target) return
    event.preventDefault()
    target.focus()
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[420px]" onOpenAutoFocus={focusInitial}>
        <div className="flex flex-col gap-2 p-5">
          <DialogTitle className="type-h3 text-foreground">{title}</DialogTitle>
          {description && (
            <DialogDescription className="text-[12px] leading-relaxed text-[hsl(var(--text2))]">
              {description}
            </DialogDescription>
          )}
          <div className="mt-3 flex justify-end gap-2">
            <Button ref={cancelRef} variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              {cancelLabel}
            </Button>
            {hasSecondary && (
              <Button
                ref={secondaryRef}
                variant="outline"
                size="sm"
                onClick={() => {
                  onSecondary()
                  onOpenChange(false)
                }}
              >
                {secondaryLabel}
              </Button>
            )}
            <Button
              ref={confirmRef}
              variant={destructive ? 'destructive' : 'default'}
              size="sm"
              data-destructive={destructive ? 'true' : undefined}
              onClick={() => {
                onConfirm()
                onOpenChange(false)
              }}
            >
              {confirmLabel}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
