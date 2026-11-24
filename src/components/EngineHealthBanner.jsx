import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { AlertTriangle, RefreshCw, Settings, X } from 'lucide-react'
import { Button } from './ui/button'

// Focus target when the banner closes while it holds focus (Dismiss, or a
// successful Re-check), so keyboard focus is not dropped to <body>.
function focusWorkspace() {
  const main = document.querySelector('main')
  if (!main) return
  if (!main.hasAttribute('tabindex')) main.setAttribute('tabindex', '-1')
  main.focus({ preventScroll: true })
}

// Shown on Convert and Player only when the audio engine check found a
// problem (src/lib/engineHealth.js). Dismissing hides it for this session;
// it returns at the next launch, or the next failed re-check, until the
// engine is fixed. Conversions stay blocked by the backend either way.
//
// Screen readers: only the title/action line is a live alert, and only the
// first time a given problem is shown (`announce`). Switching between
// Convert and Player remounts the banner without re-announcing it, and the
// Re-check button's changing label sits outside the alert; progress goes to
// a separate polite status.
export default function EngineHealthBanner({
  problem,
  onRecheck,
  onOpenDetails,
  onDismiss,
  onAnnounced,
  announce = true,
  rechecking = false,
}) {
  const rootRef = useRef(null)
  const problemId = problem?.id
  // Fixed for the life of this mount, so the alert role is never removed
  // from under a screen reader that is still reading it. App keys the banner
  // by problem id, so a different problem gets a fresh mount.
  const [isAlert] = useState(announce)

  useEffect(() => {
    if (problemId && isAlert) onAnnounced?.(problemId)
  }, [problemId, isAlert, onAnnounced])

  useLayoutEffect(() => {
    const root = rootRef.current
    return () => {
      if (root && root.contains(document.activeElement)) {
        // Unmounting: the DOM node is about to go. Move focus once it has.
        setTimeout(() => {
          if (!document.activeElement || document.activeElement === document.body) focusWorkspace()
        }, 0)
      }
    }
  }, [problemId])

  if (!problem) return null
  return (
    <div
      ref={rootRef}
      data-testid="engine-health-banner"
      className="mx-5 md:mx-8 mt-3 flex items-start gap-2.5 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2.5 type-small text-foreground"
    >
      <AlertTriangle aria-hidden="true" size={15} className="mt-px shrink-0 text-[hsl(var(--warning))]" />
      <div className="flex-1 min-w-0">
        <p role={isAlert ? 'alert' : undefined} data-testid="engine-health-message">
          <span className="font-semibold">{problem.title}</span> {problem.action}
        </p>
        {problem.detail && (
          <details className="mt-1.5">
            <summary className="w-fit cursor-pointer font-semibold text-[hsl(var(--text2))]">Technical details</summary>
            <p className="mt-1 whitespace-pre-wrap break-words type-small text-[hsl(var(--sub))]">{problem.detail}</p>
          </details>
        )}
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            aria-disabled={rechecking || undefined}
            aria-busy={rechecking || undefined}
            className={rechecking ? 'opacity-60 cursor-progress' : undefined}
            onClick={() => {
              if (!rechecking) onRecheck?.()
            }}
          >
            <RefreshCw
              size={12}
              aria-hidden="true"
              className={rechecking ? 'animate-spin motion-reduce:animate-none' : ''}
            />
            Re-check
          </Button>
          <Button size="sm" variant="ghost" onClick={onOpenDetails}>
            <Settings size={12} aria-hidden="true" /> Engine details
          </Button>
          <span role="status" className="sr-only">
            {rechecking ? 'Checking the audio engine…' : ''}
          </span>
        </div>
      </div>
      <button
        type="button"
        className="rounded-sm p-1 text-[hsl(var(--sub))] hover:text-foreground"
        aria-label="Dismiss audio engine warning for this session"
        onClick={onDismiss}
      >
        <X aria-hidden="true" className="h-3.5 w-3.5" />
      </button>
    </div>
  )
}
