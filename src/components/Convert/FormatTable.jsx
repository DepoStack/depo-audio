import { useState } from 'react'
import { FORMAT_ROWS } from '../../constants'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { Card, CardTitle } from '../ui/card'

export default function FormatTable() {
  const [open, setOpen] = useState(false)
  return (
    <Card>
      <button className="w-full flex items-center justify-between px-4 py-2.5 cursor-pointer transition-colors hover:bg-secondary/50"
        onClick={() => setOpen(o => !o)}>
        <CardTitle>FORMAT SUPPORT</CardTitle>
        {open ? <ChevronDown className="h-3.5 w-3.5 text-[hsl(var(--sub))]" /> : <ChevronRight className="h-3.5 w-3.5 text-[hsl(var(--sub))]" />}
      </button>
      {open && (
        <div className="border-t border-border/60">
          {FORMAT_ROWS.map((r, i) => (
            <div key={i} className="grid grid-cols-[170px_1fr_60px_130px] items-center gap-3 px-4 py-2 border-b border-border/60 last:border-b-0 transition-colors hover:bg-secondary/50">
              <span className="font-mono text-[11px] text-foreground whitespace-nowrap">{r.ext}</span>
              <span className="text-[11px] text-[hsl(var(--sub))]">{r.vendor}</span>
              <span className="font-mono text-[10px] text-[hsl(var(--sub))] text-right">{r.ch}</span>
              <span className={`font-mono text-[10px] font-semibold text-right ${r.status === 'supported' ? 'text-success' : r.status === 'experimental' ? 'text-warning' : 'text-destructive'}`}>
                {r.status === 'supported' ? '● Supported' : r.status === 'experimental' ? '◐ Experimental' : '✕ Cannot convert'}
              </span>
            </div>
          ))}
          <div className="px-4 py-2 text-[11px] text-[hsl(var(--sub))] border-t border-border/60 bg-secondary">
            ✕ Eclipse <code className="font-mono text-[10px] text-[hsl(var(--text2))]">.aes</code> files are AES-128 encrypted. In Eclipse: File → Export Audio → WAV, then drop that file here.
          </div>
        </div>
      )}
    </Card>
  )
}
