import { describe, expect, it } from 'vitest'
import { resolveCanvasColor } from '../lib/canvasColor'

describe('resolveCanvasColor', () => {
  const styles = {
    getPropertyValue(name) {
      return name === '--loop-fill' ? '266 23% 92% / 0.14' : ''
    },
  }

  it('resolves CSS variables before assigning them to Canvas', () => {
    expect(resolveCanvasColor('hsl(var(--loop-fill))', styles)).toBe('hsl(266 23% 92% / 0.14)')
  })

  it('carries an alpha on an opaque token', () => {
    const opaque = { getPropertyValue: () => '266 23% 92%' }
    expect(resolveCanvasColor('hsl(var(--foreground) / 0.16)', opaque)).toBe('hsl(266 23% 92% / 0.16)')
  })

  it('passes concrete colors through and falls back for missing tokens', () => {
    expect(resolveCanvasColor('#123456', styles)).toBe('#123456')
    expect(resolveCanvasColor('hsl(var(--missing))', styles, '#888')).toBe('#888')
  })
})
