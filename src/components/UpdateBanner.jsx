import { Download, X, Loader2, RefreshCw } from 'lucide-react'
import { Button } from './ui/button'

// Slim banner shown under the title bar when an update is ready to install.
// Only renders for the actionable states — idle/uptodate show nothing here
// (the Settings panel surfaces those).
export default function UpdateBanner({ updater }) {
  const { update, status, progress, installUpdate, dismiss } = updater
  if (!update || status === 'idle' || status === 'uptodate' || status === 'checking') return null

  return (
    <div className="shrink-0 flex items-center gap-3 px-5 py-2 bg-[hsl(var(--gold-dim))] border-b border-primary/30 text-[12px]">
      {status === 'available' && (
        <>
          <RefreshCw size={14} className="text-primary shrink-0" />
          <span className="text-foreground">
            <strong className="font-semibold">DepoAudio {update.version}</strong> is available
            {update.currentVersion ? ` — you have ${update.currentVersion}` : ''}.
          </span>
          <div className="ml-auto flex items-center gap-2">
            <Button size="sm" variant="primary" onClick={installUpdate}>
              <Download size={12} /> Update &amp; restart
            </Button>
            <button onClick={dismiss} aria-label="Dismiss update notice"
              className="text-[hsl(var(--sub))] hover:text-foreground transition-colors">
              <X size={14} />
            </button>
          </div>
        </>
      )}

      {status === 'downloading' && (
        <>
          <Loader2 size={14} className="text-primary animate-spin shrink-0" />
          <span className="text-foreground">Downloading update… {Math.round(progress * 100)}%</span>
          <div className="ml-auto w-40 h-1.5 bg-border rounded-full overflow-hidden">
            <div className="h-full bg-primary rounded-full transition-all" style={{ width: `${Math.round(progress * 100)}%` }} />
          </div>
        </>
      )}

      {status === 'ready' && (
        <>
          <Loader2 size={14} className="text-primary animate-spin shrink-0" />
          <span className="text-foreground">Update installed — restarting…</span>
        </>
      )}

      {status === 'error' && (
        <>
          <X size={14} className="text-destructive shrink-0" />
          <span className="text-foreground">Update failed. Please try again or download the latest version manually.</span>
          <button onClick={dismiss} aria-label="Dismiss"
            className="ml-auto text-[hsl(var(--sub))] hover:text-foreground transition-colors">
            <X size={14} />
          </button>
        </>
      )}
    </div>
  )
}
