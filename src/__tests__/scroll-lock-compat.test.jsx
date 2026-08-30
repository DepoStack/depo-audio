import { StrictMode } from 'react'
import { cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import {
  RemoveScrollBar,
  fullWidthClassName,
  getGapWidth,
  lockAttribute,
  noScrollbarsClassName,
  removedBarSizeVariable,
  zeroRightClassName,
} from '../lib/scroll-lock-compat'

const originalBodyClass = document.body.className
const originalBodyStyle = document.body.getAttribute('style')
const originalLockAttribute = document.body.getAttribute(lockAttribute)

function setViewport(innerWidth, clientWidth) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: innerWidth })
  Object.defineProperty(document.documentElement, 'clientWidth', { configurable: true, value: clientWidth })
}

afterEach(() => {
  cleanup()
  document.body.className = originalBodyClass
  if (originalBodyStyle === null) document.body.removeAttribute('style')
  else document.body.setAttribute('style', originalBodyStyle)
  if (originalLockAttribute === null) document.body.removeAttribute(lockAttribute)
  else document.body.setAttribute(lockAttribute, originalLockAttribute)
  document.querySelectorAll('style[data-depoaudio-scroll-lock]').forEach(style => style.remove())
})

describe('DepoAudio scroll-lock compatibility', () => {
  it('locks once for nested consumers and restores pre-existing body state', () => {
    setViewport(1000, 980)
    document.body.style.marginRight = '5px'
    document.body.style.overflow = 'auto'
    document.body.setAttribute(lockAttribute, 'legacy')

    const first = render(<RemoveScrollBar />)
    expect(document.body.getAttribute(lockAttribute)).toBe('1')
    expect(document.body.style.getPropertyValue('overflow')).toBe('hidden')
    expect(document.body.style.getPropertyPriority('overflow')).toBe('important')
    expect(document.body.style.marginRight).toBe('25px')
    expect(document.body.style.getPropertyValue(removedBarSizeVariable)).toBe('20px')
    expect(document.body).toHaveClass(noScrollbarsClassName)

    const second = render(<RemoveScrollBar />)
    expect(document.body.getAttribute(lockAttribute)).toBe('2')
    expect(document.body.style.marginRight).toBe('25px')

    first.unmount()
    expect(document.body.getAttribute(lockAttribute)).toBe('1')
    second.unmount()
    expect(document.body.getAttribute(lockAttribute)).toBe('legacy')
    expect(document.body.style.marginRight).toBe('5px')
    expect(document.body.style.overflow).toBe('auto')
    expect(document.body.style.getPropertyValue(removedBarSizeVariable)).toBe('')
    expect(document.body).not.toHaveClass(noScrollbarsClassName)
  })

  it('uses fractional padding compensation and exposes the compatibility constants', () => {
    setViewport(1000.5, 980.25)
    document.body.style.paddingRight = '3.5px'

    const view = render(<RemoveScrollBar gapMode="padding" noRelative />)
    expect(document.body.style.paddingRight).toBe('23.75px')
    expect(getGapWidth('padding')).toEqual({ left: 0, top: 0, right: 23.75, gap: 20.25 })
    expect(zeroRightClassName).toBe('right-scroll-bar-position')
    expect(fullWidthClassName).toBe('width-before-scroll-bar')
    expect(document.querySelector('style[data-depoaudio-scroll-lock]')).not.toBeNull()

    view.unmount()
    expect(document.body.style.paddingRight).toBe('3.5px')
    expect(document.querySelector('style[data-depoaudio-scroll-lock]')).toBeNull()
  })

  it('is safe under React Strict Mode mount and cleanup replay', () => {
    setViewport(900, 900)
    const view = render(
      <StrictMode>
        <RemoveScrollBar />
      </StrictMode>,
    )

    expect(document.body.getAttribute(lockAttribute)).toBe('1')
    expect(document.body.style.marginRight).toBe('')
    view.unmount()
    expect(document.body.hasAttribute(lockAttribute)).toBe(false)
    expect(document.body.style.overflow).toBe('')
  })
})
