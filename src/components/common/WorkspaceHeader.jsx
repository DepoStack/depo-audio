import { ShieldCheck } from 'lucide-react'
import { cn } from '../../lib/utils'

export default function WorkspaceHeader({ eyebrow, title, description, status, actions, className }) {
  return (
    <header
      className={cn(
        'workspace-header flex flex-col gap-3 border-l-2 border-foreground pl-4 sm:flex-row sm:items-end sm:justify-between',
        className,
      )}
    >
      <div className="min-w-0">
        {eyebrow && <p className="mb-1 type-label text-[hsl(var(--sub))]">{eyebrow}</p>}
        <h2 className="type-h3 text-foreground">{title}</h2>
        {description && (
          <p className="mt-1 max-w-[65ch] text-[12px] leading-relaxed text-[hsl(var(--text2))]">{description}</p>
        )}
      </div>
      {(status || actions) && (
        <div className="flex shrink-0 flex-wrap items-center gap-2 sm:justify-end">
          {status && (
            <span className="inline-flex items-center gap-1.5 rounded-md border border-border/80 bg-[hsl(var(--surface))] px-2.5 py-1.5 type-small font-medium text-[hsl(var(--text2))]">
              <ShieldCheck aria-hidden="true" className="h-3.5 w-3.5 text-success" strokeWidth={1.8} />
              {status}
            </span>
          )}
          {actions}
        </div>
      )}
    </header>
  )
}
