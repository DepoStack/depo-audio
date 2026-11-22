import { useRef } from 'react'
import { cn } from '../../lib/utils'
import { PANE_LIMITS, RESIZE_STEP } from '../../lib/playerLayout'

// A vertical splitter between a side pane and the transcript. Drag it, or
// focus it and use ←/→ (Home/End for the smallest and largest size). The
// value is the side pane's width in pixels. `onChange` follows the drag;
// `onCommit` is called once the size is settled, to save it.
export default function PaneSplitter({ side, label, controls, value, onChange, onCommit }) {
  const { min, max } = PANE_LIMITS[side]
  const drag = useRef(null)
  // The left pane grows to the right; the right pane grows to the left.
  const grow = side === 'left' ? 1 : -1
  const clamp = width => Math.round(Math.min(max, Math.max(min, width)))

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
    onCommit(clamp(keys[event.key]))
  }

  const onPointerDown = event => {
    if (event.button !== 0) return
    event.preventDefault()
    event.currentTarget.setPointerCapture?.(event.pointerId)
    drag.current = { x: event.clientX, width: value, last: value }
  }
  const onPointerMove = event => {
    if (!drag.current) return
    const width = clamp(drag.current.width + grow * (event.clientX - drag.current.x))
    drag.current.last = width
    onChange(width)
  }
  const endDrag = event => {
    if (!drag.current) return
    event.currentTarget.releasePointerCapture?.(event.pointerId)
    const width = drag.current.last
    drag.current = null
    onCommit(width)
  }

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-controls={controls}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuetext={`${value} pixels wide`}
      tabIndex={0}
      title={`${label}: drag, or use the arrow keys`}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      className={cn(
        'group relative min-h-0 cursor-col-resize touch-none select-none bg-transparent',
        'focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-foreground',
      )}
    >
      <span
        aria-hidden="true"
        className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border transition-colors group-hover:w-0.5 group-hover:bg-foreground group-focus-visible:bg-foreground"
      />
    </div>
  )
}
