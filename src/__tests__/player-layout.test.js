import { describe, expect, it } from 'vitest'
import {
  COLLAPSED_WIDTH,
  DEFAULT_LAYOUT,
  LAYOUT_KEY,
  NARROW_BREAKPOINT,
  PANE_LIMITS,
  clampPane,
  isNarrow,
  loadLayout,
  paneRange,
  paneWidths,
  readLayout,
  resizeLayout,
  writeLayout,
} from '../lib/playerLayout'

function memoryStorage(initial = {}) {
  const data = new Map(Object.entries(initial))
  return {
    data,
    getItem: key => (data.has(key) ? data.get(key) : null),
    setItem: (key, value) => data.set(key, String(value)),
  }
}

describe('player layout persistence', () => {
  it('reads the default for missing, broken or odd values', () => {
    expect(loadLayout(null)).toEqual(DEFAULT_LAYOUT)
    expect(loadLayout('not json')).toEqual(DEFAULT_LAYOUT)
    expect(loadLayout('[1,2]')).toEqual(DEFAULT_LAYOUT)
    expect(loadLayout('{"left":"wide","right":null}')).toEqual(DEFAULT_LAYOUT)
  })

  it('keeps sizes inside the pane limits', () => {
    expect(loadLayout('{"left":10,"right":9999}')).toMatchObject({
      left: PANE_LIMITS.left.min,
      right: PANE_LIMITS.right.max,
    })
    expect(clampPane('left', 250.4)).toBe(250)
  })

  it('round-trips sizes and collapsed panes through storage', () => {
    const storage = memoryStorage()
    const layout = { left: 280, right: 340, leftOpen: false, rightOpen: true }
    expect(writeLayout(storage, layout)).toBe(true)
    expect(JSON.parse(storage.data.get(LAYOUT_KEY))).toEqual(layout)
    expect(readLayout(storage)).toEqual({ layout, error: false })
  })

  it('reports storage that cannot be read or written', () => {
    const blocked = {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
    }
    expect(readLayout(blocked)).toEqual({ layout: DEFAULT_LAYOUT, error: true })
    expect(writeLayout(blocked, DEFAULT_LAYOUT)).toBe(false)
    expect(writeLayout(null, DEFAULT_LAYOUT)).toBe(false)
  })
})

describe('pane widths', () => {
  it('treats windows under 1100px as narrow', () => {
    expect(NARROW_BREAKPOINT).toBe(1100)
    expect(isNarrow(900)).toBe(true)
    expect(isNarrow(1099)).toBe(true)
    expect(isNarrow(1100)).toBe(false)
    expect(isNarrow(1920)).toBe(false)
  })

  it.each([920, 968, 1250, 1730])('gives the transcript the largest share of a %ipx workspace', total => {
    for (const layout of [
      DEFAULT_LAYOUT,
      { ...DEFAULT_LAYOUT, left: PANE_LIMITS.left.max, right: PANE_LIMITS.right.max },
    ]) {
      const { left, centre, right } = paneWidths(layout, total)
      expect(centre).toBeGreaterThan(left)
      expect(centre).toBeGreaterThan(right)
      expect(centre).toBeGreaterThan(left + right)
      expect(centre).toBeGreaterThanOrEqual(Math.floor(total * (total >= 1250 ? 0.6 : 0.55)) - 1)
      expect(left).toBeGreaterThanOrEqual(PANE_LIMITS.left.min)
      expect(right).toBeGreaterThanOrEqual(PANE_LIMITS.right.min)
    }
  })

  it('keeps the chosen sizes when there is room, and a thin rail for a collapsed pane', () => {
    expect(paneWidths(DEFAULT_LAYOUT, 1920)).toMatchObject({ left: 200, right: 260 })
    const collapsed = paneWidths({ ...DEFAULT_LAYOUT, leftOpen: false }, 1440)
    expect(collapsed.left).toBe(COLLAPSED_WIDTH)
    expect(collapsed.centre).toBe(1440 - COLLAPSED_WIDTH - 260 - 6)
  })

  it('gives the transcript most of the room at the smallest pane width', () => {
    // 1160px window less the 192px app navigation
    const { left, centre, right } = paneWidths(DEFAULT_LAYOUT, 968)
    expect(centre).toBeGreaterThan(left + right + 2 * 6)
    expect(centre / 968).toBeGreaterThan(0.55)
  })

  it('reports the room a pane really has, and saves the sizes shown when one is resized', () => {
    // 1248px workspace: room for 487px of side panes and splitters
    const total = 1248
    expect(paneRange(DEFAULT_LAYOUT, total, 'left')).toEqual({ min: 160, max: 227 })
    const wider = resizeLayout(DEFAULT_LAYOUT, total, 'left', 999)
    expect(wider).toMatchObject({ left: 227, right: 260 })
    expect(paneWidths(wider, total)).toMatchObject({ left: 227, right: 260 })
    // A squeezed pane: the stored size is above what is shown, and resizing
    // the other side saves what is shown, so nothing jumps later.
    const squeezed = { ...DEFAULT_LAYOUT, left: 320, right: 380 }
    const shown = paneWidths(squeezed, total)
    const next = resizeLayout(squeezed, total, 'right', shown.right - 16)
    expect(next.left).toBe(shown.left)
    expect(paneWidths(next, total)).toMatchObject({ left: shown.left, right: shown.right - 16 })
    // No room at all: the range is a single size.
    const tight = paneRange(DEFAULT_LAYOUT, 968, 'left')
    expect(tight.max).toBe(tight.min)
  })
})
