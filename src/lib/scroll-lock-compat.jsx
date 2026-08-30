import * as React from 'react'

/* eslint-disable react-refresh/only-export-components -- the compatibility contract intentionally exports constants beside the component */

// This small, DepoAudio-owned compatibility layer replaces the emitted
// react-remove-scroll-bar helper. Radix still owns the modal/select focus,
// keyboard, inertness, and outside-scroll behavior through react-remove-scroll.
export const zeroRightClassName = 'right-scroll-bar-position'
export const fullWidthClassName = 'width-before-scroll-bar'
export const noScrollbarsClassName = 'with-scroll-bars-hidden'
export const removedBarSizeVariable = '--removed-body-scroll-bar-size'
export const lockAttribute = 'data-scroll-locked'

const styleProperties = ['overflow', 'padding-right', 'margin-right', 'position', removedBarSizeVariable]
const locks = new Map()
let baseline = null
let ownedStyleElement = null

function readInlineStyle(element, property) {
  return {
    value: element.style.getPropertyValue(property),
    priority: element.style.getPropertyPriority(property),
  }
}

function restoreInlineStyle(element, property, saved) {
  if (saved.value) {
    element.style.setProperty(property, saved.value, saved.priority)
  } else {
    element.style.removeProperty(property)
  }
}

function measureScrollbarGap() {
  if (typeof window === 'undefined' || typeof document === 'undefined') return 0
  return Math.max(0, window.innerWidth - document.documentElement.clientWidth)
}

function pxValue(value) {
  const parsed = Number.parseFloat(value)
  return Number.isFinite(parsed) ? parsed : 0
}

export function getGapWidth(gapMode = 'margin') {
  if (typeof window === 'undefined' || typeof document === 'undefined' || !document.body) {
    return { left: 0, top: 0, right: 0, gap: 0 }
  }

  const computed = window.getComputedStyle(document.body)
  const gap = measureScrollbarGap()
  return {
    left: pxValue(computed.marginLeft),
    top: pxValue(computed.marginTop),
    right: pxValue(gapMode === 'padding' ? computed.paddingRight : computed.marginRight),
    gap,
  }
}

function captureBaseline() {
  const body = document.body
  const styles = Object.fromEntries(styleProperties.map(property => [property, readInlineStyle(body, property)]))
  const computed = window.getComputedStyle(body)

  return {
    styles,
    computed: {
      marginRight: pxValue(computed.marginRight),
      paddingRight: pxValue(computed.paddingRight),
      position: computed.position,
    },
    lockAttribute: body.getAttribute(lockAttribute),
    hadNoScrollbarsClass: body.classList.contains(noScrollbarsClassName),
  }
}

function restoreBaseline() {
  if (!baseline || typeof document === 'undefined' || !document.body) return

  const body = document.body
  for (const property of styleProperties) {
    restoreInlineStyle(body, property, baseline.styles[property])
  }

  if (baseline.lockAttribute === null) {
    body.removeAttribute(lockAttribute)
  } else {
    body.setAttribute(lockAttribute, baseline.lockAttribute)
  }

  body.classList.toggle(noScrollbarsClassName, baseline.hadNoScrollbarsClass)
}

function ensureCompatibilityStyles() {
  const existing = document.querySelector('style[data-depoaudio-scroll-lock]')
  if (existing) return

  ownedStyleElement = document.createElement('style')
  ownedStyleElement.dataset.depoaudioScrollLock = ''
  ownedStyleElement.textContent = `
.${zeroRightClassName}{right:var(${removedBarSizeVariable},0px)!important}
.${fullWidthClassName}{width:calc(100% - var(${removedBarSizeVariable},0px))!important}
`
  document.head.appendChild(ownedStyleElement)
}

function removeCompatibilityStyles() {
  ownedStyleElement?.remove()
  ownedStyleElement = null
}

function applyLocks() {
  if (typeof document === 'undefined' || !document.body || !locks.size) return

  restoreBaseline()
  const body = document.body
  const options = Array.from(locks.values()).at(-1)
  const gap = measureScrollbarGap()
  const property = options.gapMode === 'padding' ? 'padding-right' : 'margin-right'
  const base = options.gapMode === 'padding' ? baseline.computed.paddingRight : baseline.computed.marginRight

  body.style.setProperty('overflow', 'hidden', 'important')
  body.style.setProperty(removedBarSizeVariable, `${gap}px`)
  if (gap > 0) body.style.setProperty(property, `${base + gap}px`)
  if (!options.noRelative && (!baseline.computed.position || baseline.computed.position === 'static')) {
    body.style.setProperty('position', 'relative')
  }
  body.setAttribute(lockAttribute, String(locks.size))
  body.classList.add(noScrollbarsClassName)
  ensureCompatibilityStyles()
}

function acquireLock(token, options) {
  if (typeof document === 'undefined' || !document.body) return
  if (!baseline) baseline = captureBaseline()
  locks.set(token, options)
  applyLocks()
}

function releaseLock(token) {
  if (!locks.delete(token)) return
  if (locks.size) {
    applyLocks()
    return
  }

  restoreBaseline()
  baseline = null
  removeCompatibilityStyles()
}

export function useLockAttribute() {
  const token = React.useRef(Symbol('depoaudio-scroll-lock'))
  React.useLayoutEffect(() => {
    const lockToken = token.current
    acquireLock(lockToken, { gapMode: 'margin', noRelative: false })
    return () => releaseLock(lockToken)
  }, [])
}

export function RemoveScrollBar({ gapMode = 'margin', noRelative = false }) {
  const token = React.useRef(Symbol('depoaudio-scroll-lock'))

  React.useLayoutEffect(() => {
    const lockToken = token.current
    acquireLock(lockToken, { gapMode, noRelative })
    return () => releaseLock(lockToken)
  }, [gapMode, noRelative])

  return null
}
