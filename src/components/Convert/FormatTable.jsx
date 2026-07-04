import { useState } from 'react'
import { FORMAT_ROWS } from '../../constants'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { Card, CardTitle } from '../ui/card'

export default function FormatTable() {
  const [open, setOpen] = useState(false)
  const standard = FORMAT_ROWS.filter(r => r.group === 'standard')
  const court = FORMAT_ROWS.filter(r => r.group === 'court')

  return (
    <Card>
      <button className="w-full flex items-center justify-between px-4 py-2.5 cursor-pointer transition-colors hover:bg-secondary/50"
        onClick={() => setOpen(o => !o)}>
        <CardTitle>SUPPORTED FORMATS</CardTitle>
        {open ? <ChevronDown className="h-3.5 w-3.5 text-[hsl(var(--sub))]" /> : <ChevronRight className="h-3.5 w-3.5 text-[hsl(var(--sub))]" />}
      </button>
      {open && (
        <div className="border-t border-border/60">
          <div className="px-4 py-1.5 bg-secondary/50">
            <span className="font-mono text-[9px] tracking-wider uppercase text-[hsl(var(--sub))]">Standard — play, import, and convert</span>
          </div>
          {standard.map((r, i) => (
            <FormatRow key={i} r={r} />
          ))}
          <div className="px-4 py-1.5 bg-secondary/50 border-t border-border/60">
            <span className="font-mono text-[9px] tracking-wider uppercase text-[hsl(var(--sub))]">Court reporting — conversion required</span>
          </div>
          {court.map((r, i) => (
            <FormatRow key={i} r={r} />
          ))}
          <div className="px-4 py-2 text-[11px] text-[hsl(var(--sub))] border-t border-border/60 bg-secondary">
            ✕ Eclipse <code className="font-mono text-[10px] text-[hsl(var(--text2))]">.aes</code> and Liberty <code className="font-mono text-[10px] text-[hsl(var(--text2))]">.dcr</code> files must be exported to WAV from their native software first.
          </div>
        </div>
      )}
    </Card>
  )
}

function FormatRow({ r }) {
  return (
    <div className="grid grid-cols-[140px_1fr_60px_130px] items-center gap-3 px-4 py-1.5 border-b border-border/60 last:border-b-0 transition-colors hover:bg-secondary/50">
      <span className="font-mono text-[11px] text-foreground whitespace-nowrap">{r.ext}</span>
      <span className="text-[11px] text-[hsl(var(--sub))]">{r.vendor}</span>
      <span className="font-mono text-[10px] text-[hsl(var(--sub))] text-right">{r.ch}</span>
      <span className={`font-mono text-[10px] font-semibold text-right ${r.status === 'supported' ? 'text-success' : r.status === 'experimental' ? 'text-warning' : 'text-destructive'}`}>
        {r.status === 'supported' ? '● Supported' : r.status === 'experimental' ? '◐ Experimental' : '✕ Export first'}
      </span>
    </div>
  )
}
