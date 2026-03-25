import { cn } from '../../lib/utils'
import { Badge } from '../ui/badge'
import { Switch } from '../ui/switch'

export default function ProcessingToggle({ name, desc, checked, onChange, smart, detected, extra }) {
  return (
    <label className={cn(
      'flex items-center justify-between gap-4 px-4 py-2.5 cursor-pointer',
      'border-b border-border/60 last:border-b-0 hover:bg-secondary/50 transition-colors',
      smart && 'border-l-2 border-l-[hsl(var(--blue)/0.2)] pl-3.5'
    )}>
      <div className="flex flex-col gap-0.5">
        <span className="text-[13px] font-semibold text-foreground">{name}</span>
        <span className="text-[11px] text-[hsl(var(--sub))] leading-snug">
          {desc}
          {detected && <Badge variant="info" className="ml-1.5 align-middle">{detected}</Badge>}
        </span>
        {extra}
      </div>
      <Switch checked={checked} onCheckedChange={onChange} />
    </label>
  )
}
