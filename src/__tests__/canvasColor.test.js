import { describe, expect, it } from 'vitest'
import { resolveCanvasColor } from '../lib/canvasColor'

describe('resolveCanvasColor', () => {
  const styles = {
    getPropertyValue(name) {
      return name === '--gold-dim' ? '36 79% 57% / 0.14' : ''
    },
  }

  it('resolves CSS variables before assigning them to Canvas', () => {
    expect(resolveCanvasColor('hsl(var(--gold-dim))', styles)).toBe('hsl(36 79% 57% / 0.14)')
  })

  it('passes concrete colors through and falls back for missing tokens', () => {
    expect(resolveCanvasColor('#123456', styles)).toBe('#123456')
    expect(resolveCanvasColor('hsl(var(--missing))', styles, '#888')).toBe('#888')
  })
})
