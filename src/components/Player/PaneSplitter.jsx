import { useRef } from 'react'
import { cn } from '../../lib/utils'
import { RESIZE_STEP } from '../../lib/playerLayout'

// A vertical splitter between a side pane and the transcript. Drag it, or
// focus it and use ←/→ (Home/End for the smallest and largest size). The
// value is the side pane's width in pixels as shown; `min` and `max` are the
// sizes this window really allows (the centre keeps most of the room), so the
// splitter never offers room it cannot give. `onChange` follows the drag;
// `onCommit` is called once the size is settled, to save it.
export default function PaneSplitter({ side, label, controls, value, min, max, onChange, onCommit }) {
  const drag = useRef(null)
  // The left pane grows to the right; the right pane grows to the left.
  const grow = side === 'left' ? 1 : -1
  const clamp = width => Math.round(Math.min(max, Math.max(min, width)))
  const fixed = max <= min

  const onKeyDown = event => {
    const keys = {
      ArrowRight: value + grow * RESIZE_STEP,
      ArrowLeft: value - grow * RESIZE_STEP,
      Home: min,
      End: max,
    }
    if (!(event.key in keys) || event.metaKey || event.ctrlKey || event.altKey) return
    // The arrows also seek the audio everywhere else in the Player.
    event.preventDefault()
    event.stopPropagation()
    const next = clamp(keys[event.key])
    // At the edge nothing changes, and the saved size is left as it is.
    if (next !== value) onCommit(next)
  }

  const onPointerDown = event => {
    if (event.button !== 0 || fixed) return
    event.preventDefault()
    event.currentTarget.setPointerCapture?.(event.pointerId)
    drag.current = { x: event.clientX, width: value, last: value }
  }
  const onPointerMove = event => {
    if (!drag.current) return
    const width = clamp(drag.current.width + grow * (event.clientX - drag.current.x))
    if (width === drag.current.last) return
    drag.current.last = width
    onChange(width)
  }
  const endDrag = event => {
    if (!drag.current) return
    event.currentTarget.releasePointerCapture?.(event.pointerId)
    const { width, last } = drag.current
    drag.current = null
    if (last !== width) onCommit(last)
  }

  const edge = fixed
    ? ', no room to resize in this window'
    : value >= max
      ? ', the widest it can be in this window'
      : value <= min
        ? ', the narrowest it can be'
        : ''

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-controls={controls}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuetext={`${value} pixels wide${edge}`}
      tabIndex={0}
      title={fixed ? `${label}: no room to resize in this window` : `${label}: drag, or use the arrow keys`}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      data-fixed={fixed ? 'true' : undefined}
      className={cn(
        // The pointer target reaches 4px past each side of the 6px column.
        'group relative z-10 min-h-0 touch-none select-none bg-transparent',
        "before:absolute before:inset-y-0 before:-left-1 before:-right-1 before:content-['']",
        fixed ? 'cursor-default' : 'cursor-col-resize',
        'focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-foreground',
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          'absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border transition-colors',
          !fixed && 'group-hover:bg-foreground group-focus-visible:bg-foreground',
        )}
      />
      {/* A grip, shown on hover and focus, so the splitter reads as a handle. */}
      {!fixed && (
        <span
          aria-hidden="true"
          className="absolute left-1/2 top-1/2 flex h-9 w-2.5 -translate-x-1/2 -translate-y-1/2 flex-col items-center justify-center gap-1 rounded-full border border-foreground bg-card opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
        >
          <span className="h-0.5 w-0.5 rounded-full bg-foreground" />
          <span className="h-0.5 w-0.5 rounded-full bg-foreground" />
          <span className="h-0.5 w-0.5 rounded-full bg-foreground" />
        </span>
      )}
    </div>
  )
}
