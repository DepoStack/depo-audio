import { describe, expect, it } from 'vitest'
import { shouldIgnoreNavigationShortcut } from '../lib/utils'

function keyEvent(target, overrides = {}) {
  return { target, metaKey: false, ctrlKey: false, altKey: false, ...overrides }
}

describe('global navigation shortcuts', () => {
  it('does not intercept keys from interactive controls', () => {
    const doc = document.implementation.createHTMLDocument()
    const button = doc.createElement('button')
    doc.body.append(button)

    expect(shouldIgnoreNavigationShortcut(keyEvent(button), doc)).toBe(true)
  })

  it('does not switch tabs while any dialog is open', () => {
    const doc = document.implementation.createHTMLDocument()
    const dialog = doc.createElement('div')
    dialog.setAttribute('role', 'dialog')
    doc.body.append(dialog)

    expect(shouldIgnoreNavigationShortcut(keyEvent(doc.body), doc)).toBe(true)
  })

  it('allows an unmodified number key from a non-interactive target', () => {
    const doc = document.implementation.createHTMLDocument()

    expect(shouldIgnoreNavigationShortcut(keyEvent(doc.body), doc)).toBe(false)
    expect(shouldIgnoreNavigationShortcut(keyEvent(doc.body, { ctrlKey: true }), doc)).toBe(true)
  })
})
