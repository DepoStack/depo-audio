import { useState } from 'react'
import { FORMAT_ROWS } from '../../constants'
import { ChevronDown, ChevronRight, CircleCheck, CircleDashed, CircleX } from 'lucide-react'

export default function FormatTable() {
  const [open, setOpen] = useState(false)
  const standard = FORMAT_ROWS.filter(row => row.group === 'standard')
  const court = FORMAT_ROWS.filter(row => row.group === 'court')
  const exportFirst = FORMAT_ROWS.filter(row => row.group === 'export')

  return (
    <div className="workflow-section zone">
      <button
        type="button"
        className="group flex w-full cursor-pointer items-center justify-between gap-3 text-left"
        aria-expanded={open}
        onClick={() => setOpen(current => !current)}
      >
        <div>
          <span className="block type-label text-ink-2">Supported formats</span>
          <p className="type-small text-ink-2 group-hover:text-foreground">
            Available formats, Preview court audio, and export-first formats
          </p>
        </div>
        {open ? (
          <ChevronDown aria-hidden="true" className="h-4 w-4 shrink-0" />
        ) : (
          <ChevronRight aria-hidden="true" className="h-4 w-4 shrink-0" />
        )}
      </button>
      {open && (
        <div className="border-y border-border">
          <p className={group}>Standard — play, import, and convert</p>
          {standard.map(row => (
            <FormatRow key={`${row.ext}-${row.vendor}`} row={row} />
          ))}
          <p className={group}>Court reporting — conversion required</p>
          {court.map(row => (
            <FormatRow key={`${row.ext}-${row.vendor}`} row={row} />
          ))}
          <p className={group}>Closed formats — export to WAV first</p>
          {exportFirst.map(row => (
            <FormatRow key={`${row.ext}-${row.vendor}`} row={row} />
          ))}
          <div className="flex items-start gap-2 pr-3 py-2 type-small text-ink-2">
            <CircleX aria-hidden="true" className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" strokeWidth={1.8} />
            <span>
              DepoAudio recognizes these files but cannot open them. Export or convert them to WAV in the software that
              recorded them, then add the WAV. When you add one, DepoAudio names the software to use.
            </span>
          </div>
        </div>
      )}
    </div>
  )
}

const group = 'pt-4 pb-1 type-label text-ink-2'

function FormatRow({ row }) {
  const status =
    row.status === 'supported'
      ? { Icon: CircleCheck, label: 'Available', className: 'text-success' }
      : row.status === 'experimental'
        ? { Icon: CircleDashed, label: 'Preview', className: 'text-warning' }
        : { Icon: CircleX, label: 'Export first', className: 'text-destructive' }
  const { Icon } = status

  return (
    <div className="format-support-row grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 border-b border-border pr-3 py-2 hover:bg-secondary/50">
      <div className="min-w-0">
        <span className="block truncate type-small font-medium">{row.ext}</span>
        <span className="block truncate type-small text-ink-2">{row.vendor}</span>
        {row.note && <span className="block type-small text-ink-2">{row.note}</span>}
      </div>
      <div className="flex items-center gap-3">
        <span className="type-min tabular-nums text-ink-2">{row.ch}</span>
        <span className={`inline-flex min-w-[92px] items-center justify-end gap-1 type-label ${status.className}`}>
          <Icon aria-hidden="true" className="h-3 w-3" strokeWidth={1.8} />
          {status.label}
        </span>
      </div>
    </div>
  )
}
