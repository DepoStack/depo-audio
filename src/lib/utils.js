import { clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs) {
  return twMerge(clsx(inputs))
}

const INTERACTIVE_SHORTCUT_TARGETS =
  'input, textarea, select, button, a[href], [role="button"], [role="menuitem"], [contenteditable="true"]'

export function shouldIgnoreNavigationShortcut(event, documentRoot = document) {
  if (event.metaKey || event.ctrlKey || event.altKey) return true
  if (documentRoot?.querySelector?.('[role="dialog"]')) return true
  const target = event.target
  return Boolean(target?.isContentEditable || target?.closest?.(INTERACTIVE_SHORTCUT_TARGETS))
}
