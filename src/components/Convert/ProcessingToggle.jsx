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
        'flex items-center justify-between gap-4 px-4 py-2.5',
        'border-b border-border/60 last:border-b-0 hover:bg-secondary/50 transition-colors',
        disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer',
        smart && 'border-l-2 border-l-[hsl(var(--blue)/0.2)] pl-3.5',
      )}
    >
      <div className="flex flex-col gap-0.5">
        <Label
          htmlFor={id}
          className={cn(
            'text-[13px] font-semibold text-foreground flex items-center gap-1.5 flex-wrap',
            disabled ? 'cursor-not-allowed' : 'cursor-pointer',
          )}
        >
          {name}
          {detected && (
            <span
              className="text-[9.5px] font-semibold px-1.5 py-px rounded-full text-[hsl(var(--success))] bg-[hsl(var(--success)/0.12)]"
              title={detected}
            >
              Recommended — {detected}
            </span>
          )}
        </Label>
        <span id={descriptionId} className="text-[11px] text-[hsl(var(--sub))] leading-snug">
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
