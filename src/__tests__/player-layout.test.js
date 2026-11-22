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
  paneWidths,
  readLayout,
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
      expect(centre).toBeGreaterThanOrEqual(Math.floor(total * (total >= 968 ? 0.5 : 0.45)) - 1)
      expect(left).toBeGreaterThanOrEqual(PANE_LIMITS.left.min)
      expect(right).toBeGreaterThanOrEqual(PANE_LIMITS.right.min)
    }
  })

  it('keeps the chosen sizes when there is room, and a thin rail for a collapsed pane', () => {
    expect(paneWidths(DEFAULT_LAYOUT, 1920)).toMatchObject({ left: 220, right: 280 })
    const collapsed = paneWidths({ ...DEFAULT_LAYOUT, leftOpen: false }, 1440)
    expect(collapsed.left).toBe(COLLAPSED_WIDTH)
    expect(collapsed.centre).toBe(1440 - COLLAPSED_WIDTH - 280 - 6)
  })
})
