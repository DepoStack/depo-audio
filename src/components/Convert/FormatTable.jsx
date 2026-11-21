import { useState } from 'react'
import { FORMAT_ROWS } from '../../constants'
import { ChevronDown, ChevronRight, CircleCheck, CircleDashed, CircleX } from 'lucide-react'
import { Card, CardTitle } from '../ui/card'

export default function FormatTable() {
  const [open, setOpen] = useState(false)
  const standard = FORMAT_ROWS.filter(row => row.group === 'standard')
  const court = FORMAT_ROWS.filter(row => row.group === 'court')
  const exportFirst = FORMAT_ROWS.filter(row => row.group === 'export')

  return (
    <Card className="workflow-section">
      <button
        type="button"
        className="flex w-full cursor-pointer items-center justify-between px-4 py-3 text-left transition-colors hover:bg-secondary/50"
        aria-expanded={open}
        onClick={() => setOpen(current => !current)}
      >
        <div>
          <CardTitle as="span" className="block">
            Supported formats
          </CardTitle>
          <p className="mt-1 type-small text-[hsl(var(--sub))]">
            Direct support, experimental court audio, and export-first formats
          </p>
        </div>
        {open ? (
          <ChevronDown aria-hidden="true" className="h-3.5 w-3.5 text-[hsl(var(--sub))]" />
        ) : (
          <ChevronRight aria-hidden="true" className="h-3.5 w-3.5 text-[hsl(var(--sub))]" />
        )}
      </button>
      {open && (
        <div className="border-t border-border/60">
          <div className="bg-secondary/50 px-4 py-1.5">
            <span className="type-label text-[hsl(var(--sub))]">Standard — play, import, and convert</span>
          </div>
          {standard.map(row => (
            <FormatRow key={`${row.ext}-${row.vendor}`} row={row} />
          ))}
          <div className="border-t border-border/60 bg-secondary/50 px-4 py-1.5">
            <span className="type-label text-[hsl(var(--sub))]">Court reporting — conversion required</span>
          </div>
          {court.map(row => (
            <FormatRow key={`${row.ext}-${row.vendor}`} row={row} />
          ))}
          <div className="border-t border-border/60 bg-secondary/50 px-4 py-1.5">
            <span className="type-label text-[hsl(var(--sub))]">Closed formats — export to WAV first</span>
          </div>
          {exportFirst.map(row => (
            <FormatRow key={`${row.ext}-${row.vendor}`} row={row} />
          ))}
          <div className="flex items-start gap-2 border-t border-border/60 bg-secondary px-4 py-2 type-small text-[hsl(var(--sub))]">
            <CircleX aria-hidden="true" className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" strokeWidth={1.8} />
            <span>
              DepoAudio recognizes these files but cannot open them. Export or convert them to WAV in the software that
              recorded them, then add the WAV. When you add one, DepoAudio names the software to use.
            </span>
          </div>
        </div>
      )}
    </Card>
  )
}

function FormatRow({ row }) {
  const status =
    row.status === 'supported'
      ? { Icon: CircleCheck, label: 'Supported', className: 'text-success' }
      : row.status === 'experimental'
        ? { Icon: CircleDashed, label: 'Experimental', className: 'text-warning' }
        : { Icon: CircleX, label: 'Export first', className: 'text-destructive' }
  const { Icon } = status

  return (
    <div className="format-support-row grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 border-b border-border/60 px-4 py-2 last:border-b-0 transition-colors hover:bg-secondary/50">
      <div className="min-w-0">
        <span className="block truncate font-mono text-[12px] text-foreground">{row.ext}</span>
        <span className="block truncate type-small text-[hsl(var(--sub))]">{row.vendor}</span>
      </div>
      <div className="flex items-center gap-3">
        <span className="tabular-nums text-[12px] text-[hsl(var(--sub))]">{row.ch}</span>
        <span
          className={`inline-flex min-w-[92px] items-center justify-end gap-1 tabular-nums text-[12px] font-semibold ${status.className}`}
        >
          <Icon aria-hidden="true" className="h-3 w-3" strokeWidth={1.8} />
          {status.label}
        </span>
      </div>
    </div>
  )
}
