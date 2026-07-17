import * as React from 'react'
import { cn } from '../../lib/utils'

// Segmented control — the Docket pattern for choosing one of a few exclusive
// options: a tinted track (bg-secondary/60) with the active segment lifted on
// a card surface (bg-card shadow-sm ring). Extracted from the Convert tab's
// output-mode/bitrate controls so every surface shares one recipe.
//
// options: [{ value, label, title?, icon? }]  (label may be a node)
// size: 'sm' (compact, transport/inline) | 'md' (default)

const Segmented = React.forwardRef(function Segmented(
  { options, value, onChange, size = 'md', 'aria-label': ariaLabel, className, ...props },
  ref
) {
  return (
    <div
      ref={ref}
      role="tablist"
      aria-label={ariaLabel}
      className={cn('inline-flex items-center gap-0.5 rounded-lg bg-secondary/60 p-0.5', className)}
      {...props}
    >
      {options.map((o) => {
        const active = o.value === value
        return (
          <button
            key={o.value}
            type="button"
            role="tab"
            aria-selected={active}
            title={o.title}
            onClick={() => onChange(o.value)}
            className={cn(
              'inline-flex items-center gap-1.5 rounded-md font-medium transition-colors cursor-pointer',
              'focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
              size === 'sm' ? 'px-2 py-1 text-[11px]' : 'px-3 py-1.5 text-[12px]',
              active
                ? 'bg-card text-foreground shadow-sm ring-1 ring-border'
                : 'text-[hsl(var(--sub))] hover:text-foreground'
            )}
          >
            {o.icon}
            {o.label}
          </button>
        )
      })}
    </div>
  )
})

export { Segmented }
