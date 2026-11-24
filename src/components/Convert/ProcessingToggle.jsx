import { useId } from 'react'
import { cn } from '../../lib/utils'
import { Switch } from '../ui/switch'
import { Label } from '../ui/label'

export default function ProcessingToggle({ name, desc, checked, onChange, smart, detected, extra, disabled = false }) {
  const id = useId()
  const descriptionId = `${id}-description`
  return (
    <div
      className={cn(
        'flex items-center justify-between gap-4 pr-3 py-2 border-b border-border hover:bg-secondary/50 transition-colors',
        // Unavailable rows are drawn in ink 2, never faded.
        disabled ? 'cursor-not-allowed text-ink-2' : 'cursor-pointer',
        // Text stays on the zone edge; the recommended-group rule hangs in the gutter.
        smart && '-ml-3 border-l-2 border-l-[hsl(var(--blue)/0.2)] pl-2.5',
      )}
    >
      <div className="flex flex-col gap-0.5">
        <Label
          htmlFor={id}
          className={cn(
            'type-small font-semibold flex items-center gap-1.5 flex-wrap',
            disabled ? 'cursor-not-allowed' : 'cursor-pointer text-foreground',
          )}
        >
          {name}
          {detected && (
            <span className="type-label px-1.5 border border-success text-success" title={detected}>
              Recommended — {detected}
            </span>
          )}
        </Label>
        <span id={descriptionId} className="type-small text-ink-2">
          {desc}
        </span>
        {extra}
      </div>
      <Switch
        id={id}
        aria-describedby={descriptionId}
        checked={checked}
        disabled={disabled}
        onCheckedChange={onChange}
      />
    </div>
  )
}
