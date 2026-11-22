import { AlertTriangle, RefreshCw, Settings, X } from 'lucide-react'
import { Button } from './ui/button'

// Shown on Convert and Player only when the audio engine check found a
// problem (src/lib/engineHealth.js). Dismissing hides it for this session;
// it returns at the next launch, or the next failed re-check, until the
// engine is fixed. Conversions stay blocked by the backend either way.
export default function EngineHealthBanner({ problem, onRecheck, onOpenDetails, onDismiss, rechecking = false }) {
  if (!problem) return null
  return (
    <div
      role="alert"
      data-testid="engine-health-banner"
      className="mx-5 md:mx-8 mt-3 flex items-start gap-2.5 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2.5 type-small text-foreground"
    >
      <AlertTriangle aria-hidden="true" size={15} className="mt-px shrink-0 text-[hsl(var(--warning))]" />
      <div className="flex-1 min-w-0">
        <p>
          <span className="font-semibold">{problem.title}</span> {problem.action}
        </p>
        {problem.detail && (
          <details className="mt-1.5">
            <summary className="w-fit cursor-pointer font-semibold text-[hsl(var(--text2))]">Technical details</summary>
            <p className="mt-1 whitespace-pre-wrap break-words type-small text-[hsl(var(--sub))]">{problem.detail}</p>
          </details>
        )}
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button size="sm" variant="outline" onClick={onRecheck} disabled={rechecking}>
            <RefreshCw
              size={12}
              aria-hidden="true"
              className={rechecking ? 'animate-spin motion-reduce:animate-none' : ''}
            />
            {rechecking ? 'Checking…' : 'Re-check'}
          </Button>
          <Button size="sm" variant="ghost" onClick={onOpenDetails}>
            <Settings size={12} aria-hidden="true" /> Engine details
          </Button>
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
