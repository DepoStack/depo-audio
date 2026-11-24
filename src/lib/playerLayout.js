// ── Player pane layout ──────────────────────────────────────────────────────
//
// The Player is three panes over the transport bar: Playlist and Bookmarks on
// the left, the transcript in the centre, and Now Playing, the listening mix
// and the flags on the right. The side panes can be resized (drag or keyboard
// on the splitter) or collapsed; the sizes are kept per viewer in
// localStorage under LAYOUT_KEY. The centre always keeps the largest share.
//
// Below NARROW_BREAKPOINT the side panes become drawers the person opens and
// closes, and the transcript keeps the full width.

export const LAYOUT_KEY = 'player-layout'
export const NARROW_BREAKPOINT = 1100

export const PANE_LIMITS = {
  left: { min: 160, max: 320, initial: 200 },
  right: { min: 224, max: 380, initial: 260 },
}
// Width of a collapsed side pane (its expand button) and of a splitter. The
// splitter's pointer target reaches a few pixels past each side of it.
export const COLLAPSED_WIDTH = 40
export const SPLITTER_WIDTH = 6
// The centre gets at least 60% of the workspace (as long as the side panes
// are above their minimum), so the transcript has most of the room.
export const CENTRE_MIN_SHARE = 0.6
export const RESIZE_STEP = 16

export const DEFAULT_LAYOUT = Object.freeze({
  left: PANE_LIMITS.left.initial,
  right: PANE_LIMITS.right.initial,
  leftOpen: true,
  rightOpen: true,
})

export function clampPane(side, width) {
  const { min, max, initial } = PANE_LIMITS[side]
  const value = Number(width)
  if (!Number.isFinite(value)) return initial
  return Math.round(Math.min(max, Math.max(min, value)))
}

// A stored layout, or the default for anything missing or unreadable.
export function loadLayout(raw) {
  let parsed
  try {
    parsed = raw ? JSON.parse(raw) : null
  } catch {
    parsed = null
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return { ...DEFAULT_LAYOUT }
  return {
    left: clampPane('left', parsed.left ?? DEFAULT_LAYOUT.left),
    right: clampPane('right', parsed.right ?? DEFAULT_LAYOUT.right),
    leftOpen: parsed.leftOpen !== false,
    rightOpen: parsed.rightOpen !== false,
  }
}

export function serializeLayout(layout) {
  const { left, right, leftOpen, rightOpen } = loadLayout(JSON.stringify(layout))
  return JSON.stringify({ left, right, leftOpen, rightOpen })
}

// Reads the saved layout. `error` is true when storage could not be read.
export function readLayout(storage) {
  try {
    return { layout: loadLayout(storage?.getItem(LAYOUT_KEY)), error: false }
  } catch {
    return { layout: { ...DEFAULT_LAYOUT }, error: true }
  }
}

// Saves the layout; false when storage refused it.
export function writeLayout(storage, layout) {
  try {
    if (!storage) return false
    storage.setItem(LAYOUT_KEY, serializeLayout(layout))
    return true
  } catch {
    return false
  }
}

export function isNarrow(width) {
  return Number.isFinite(width) && width < NARROW_BREAKPOINT
}

const splittersOf = layout => SPLITTER_WIDTH * (Number(layout.leftOpen) + Number(layout.rightOpen))
const OTHER = { left: 'right', right: 'left' }

// The room both side panes (and the splitters) may take in a workspace
// `width` pixels wide.
const sideRoomOf = (layout, width) => Math.max(0, width * (1 - CENTRE_MIN_SHARE) - splittersOf(layout))

// The pixel widths the panes get in a workspace `total` pixels wide. Side
// panes shrink (towards their minimum) when the centre would otherwise fall
// below CENTRE_MIN_SHARE, and the centre is never narrower than a side pane.
export function paneWidths(layout, total) {
  const leftWanted = layout.leftOpen ? clampPane('left', layout.left) : COLLAPSED_WIDTH
  const rightWanted = layout.rightOpen ? clampPane('right', layout.right) : COLLAPSED_WIDTH
  const splitters = splittersOf(layout)
  const width = Number.isFinite(total) && total > 0 ? total : leftWanted + rightWanted + splitters + 640
  const sideRoom = sideRoomOf(layout, width)
  let left = leftWanted
  let right = rightWanted
  const over = left + right - sideRoom
  if (over > 0) {
    // Shrink the open panes in proportion to the room they have above their minimum
    const leftSlack = layout.leftOpen ? left - PANE_LIMITS.left.min : 0
    const rightSlack = layout.rightOpen ? right - PANE_LIMITS.right.min : 0
    const slack = leftSlack + rightSlack
    if (slack > 0) {
      const take = Math.min(over, slack)
      left -= Math.round((take * leftSlack) / slack)
      right -= Math.round((take * rightSlack) / slack)
    }
  }
  const centre = Math.max(0, width - left - right - splitters)
  return { left, centre, right }
}

// The sizes a side pane can really take in a workspace `total` pixels wide,
// with the other pane as it is shown now: the splitter reports these, so it
// never offers room the centre would take back.
export function paneRange(layout, total, side) {
  const { min, max } = PANE_LIMITS[side]
  const shown = paneWidths(layout, total)
  const width = shown.left + shown.centre + shown.right + splittersOf(layout)
  const room = Math.floor(sideRoomOf(layout, width) - shown[OTHER[side]])
  return { min, max: Math.max(min, Math.min(max, room)) }
}

// The layout after the person resizes `side` to `width`. The width is kept
// inside what the window allows, and the other open pane is saved at the size
// it is shown, so what is saved is what is on screen (nothing jumps later).
export function resizeLayout(layout, total, side, width) {
  const { min, max } = paneRange(layout, total, side)
  const shown = paneWidths(layout, total)
  const other = OTHER[side]
  const otherOpen = other === 'left' ? layout.leftOpen : layout.rightOpen
  const value = Number(width)
  return {
    ...layout,
    [side]: Math.round(Math.min(max, Math.max(min, Number.isFinite(value) ? value : shown[side]))),
    [other]: otherOpen ? shown[other] : layout[other],
  }
}
