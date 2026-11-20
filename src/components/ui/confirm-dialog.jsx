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
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[420px]">
        <div className="flex flex-col gap-2 p-5">
          <DialogTitle className="type-h3 text-foreground">{title}</DialogTitle>
          {description && (
            <DialogDescription className="text-[12px] leading-relaxed text-[hsl(var(--text2))]">
              {description}
            </DialogDescription>
          )}
          <div className="mt-3 flex justify-end gap-2">
            <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              {cancelLabel}
            </Button>
            {secondaryLabel && onSecondary && (
              <Button
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
              variant={destructive ? 'destructive' : 'default'}
              size="sm"
              autoFocus
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
