// ── Playback keys while typing elsewhere ─────────────────────────────────────
//
// System-wide keys for the Player, registered through the official Tauri
// global-shortcut plugin only after the person turns the setting on. This file
// holds the pure parts: which keys are allowed, how a key press in the
// recorder becomes a plugin accelerator, and how a press or release becomes a
// Player action. The hook in hooks/usePlaybackKeys.js does the registering.
//
// Accelerators use the plugin's own grammar: modifiers first, then one key,
// joined by "+", with KeyboardEvent.code names for the key ("Control+Alt+KeyP",
// "F14"). The plugin maps "Super" to Command on macOS and the Windows key on
// Windows.

export const PLAYBACK_KEY_ACTIONS = [
  { id: 'back', label: 'Back', hint: 'Backstep amount, or 2 s' },
  { id: 'playPause', label: 'Play/Pause', hint: 'Like the Play button' },
  { id: 'forward', label: 'Forward 2 s', hint: 'Skip ahead 2 s' },
  { id: 'slower', label: 'Slower', hint: 'One speed step down' },
  { id: 'faster', label: 'Faster', hint: 'One speed step up' },
]
const ACTION_IDS = new Set(PLAYBACK_KEY_ACTIONS.map(action => action.id))

// Suggested, never registered on their own: "Use suggested keys" copies them
// into the person's assignments, and nothing is registered until the setting
// is on. F13–F15 are on full-size keyboards, and pedal configuration tools for
// keyboard-mode pedals can send them. They avoid the F-keys that Express Scribe
// (F4, F7–F10) and Case CATalyst audio (Shift+F9, Ctrl+F9) use by default.
export const SUGGESTED_PLAYBACK_KEYS = { back: 'F13', playPause: 'F14', forward: 'F15' }

export const FORWARD_SECONDS = 2
export const BACK_FALLBACK_SECONDS = 2

export const DEFAULT_PLAYBACK_KEYS = { enabled: false, holdToPlay: false, bindings: {} }

const MODIFIER_ORDER = ['Control', 'Alt', 'Shift', 'Super']
const FUNCTION_KEY = /^F([1-9]|1[0-9]|2[0-4])$/
const LETTER = /^Key[A-Z]$/
const DIGIT = /^Digit[0-9]$/
const NUMPAD = /^Numpad([0-9]|Add|Decimal|Divide|Enter|Equal|Multiply|Subtract)$/
const NAMED_KEYS = new Set([
  'Backquote',
  'Backslash',
  'BracketLeft',
  'BracketRight',
  'Comma',
  'Equal',
  'Minus',
  'Period',
  'Quote',
  'Semicolon',
  'Slash',
  'Space',
  'Tab',
  'Enter',
  'Backspace',
  'Delete',
  'Insert',
  'Home',
  'End',
  'PageUp',
  'PageDown',
  'ArrowUp',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
  'Pause',
])
// Lock and screenshot keys change state for the whole system; media keys
// would need a macOS event tap, which needs Accessibility permission.
const REFUSED_KEYS = new Set(['CapsLock', 'NumLock', 'ScrollLock', 'PrintScreen'])
// Ctrl/Cmd with these is copy, paste, cut, undo, redo, select all and save in
// nearly every program; taking them system-wide would break typing.
const EDITING_LETTERS = new Set(['KeyA', 'KeyC', 'KeyS', 'KeyV', 'KeyX', 'KeyY', 'KeyZ'])
const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'OS', 'AltGraph', 'Hyper', 'Super'])

function isSupportedKeyCode(code) {
  return FUNCTION_KEY.test(code) || LETTER.test(code) || DIGIT.test(code) || NUMPAD.test(code) || NAMED_KEYS.has(code)
}

/** Split an accelerator into { modifiers, key }, or null when it is malformed. */
export function parseAccelerator(accelerator) {
  if (typeof accelerator !== 'string' || !accelerator || accelerator.length > 64) return null
  const parts = accelerator.split('+')
  const key = parts.pop()
  if (!key || !isSupportedKeyCode(key) || REFUSED_KEYS.has(key)) return null
  const modifiers = []
  for (const part of parts) {
    if (!MODIFIER_ORDER.includes(part) || modifiers.includes(part)) return null
    modifiers.push(part)
  }
  modifiers.sort((a, b) => MODIFIER_ORDER.indexOf(a) - MODIFIER_ORDER.indexOf(b))
  return { modifiers, key }
}

/**
 * Whether a key may be taken from every app. Returns '' when it may, or the
 * reason it may not, written for the person.
 */
export function playbackKeyProblem(accelerator) {
  const parsed = parseAccelerator(accelerator)
  if (!parsed) {
    if (typeof accelerator === 'string' && accelerator.split('+').pop() === 'Escape') {
      return 'Escape closes dialogs and cancels in every app, so it can’t be a playback key.'
    }
    return 'That key can’t be used as a playback key. Choose a function key, or a key with Ctrl or Cmd.'
  }
  const { modifiers, key } = parsed
  if (FUNCTION_KEY.test(key)) return ''
  const hasCommandModifier = modifiers.includes('Control') || modifiers.includes('Super')
  if (!hasCommandModifier) {
    if (key === 'Space') return 'Space types a space in every app, so it needs Ctrl or Cmd with it.'
    return 'This key types or moves the cursor in every app. Add Ctrl or Cmd, or choose a function key.'
  }
  if (EDITING_LETTERS.has(key) && modifiers.length === 1) {
    return 'That is copy, paste, undo or save in most programs. Choose another key.'
  }
  return ''
}

/**
 * Known default uses of a key in tools reporters run beside DepoAudio. A note
 * only: the person may still choose the key.
 */
export function playbackKeyNote(accelerator) {
  const parsed = parseAccelerator(accelerator)
  if (!parsed) return ''
  const name = [...parsed.modifiers, parsed.key].join('+')
  if (name === 'Shift+F9' || name === 'Control+F9') return 'Case CATalyst uses this key for its own audio playback.'
  if (['F4', 'F7', 'F8', 'F9', 'F10'].includes(name)) return 'Express Scribe uses this key by default.'
  if (name === 'F1') return 'Most programs open their help with F1.'
  return ''
}

const IS_MAC =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '')

const KEY_LABELS = {
  Backquote: '`',
  Backslash: '\\',
  BracketLeft: '[',
  BracketRight: ']',
  Comma: ',',
  Equal: '=',
  Minus: '-',
  Period: '.',
  Quote: "'",
  Semicolon: ';',
  Slash: '/',
  ArrowUp: 'Up arrow',
  ArrowDown: 'Down arrow',
  ArrowLeft: 'Left arrow',
  ArrowRight: 'Right arrow',
  PageUp: 'Page Up',
  PageDown: 'Page Down',
}

/** A readable name for an accelerator, e.g. "Ctrl+Alt+P" or "Cmd+Option+P". */
export function formatPlaybackKey(accelerator, { mac = IS_MAC } = {}) {
  const parsed = parseAccelerator(accelerator)
  if (!parsed) return accelerator || ''
  const modifierLabel = {
    Control: 'Ctrl',
    Alt: mac ? 'Option' : 'Alt',
    Shift: 'Shift',
    Super: mac ? 'Cmd' : 'Win',
  }
  const { key } = parsed
  let keyLabel = KEY_LABELS[key] ?? key
  if (LETTER.test(key)) keyLabel = key.slice(3)
  else if (DIGIT.test(key)) keyLabel = key.slice(5)
  else if (NUMPAD.test(key)) keyLabel = `Num ${key.slice(6)}`
  return [...parsed.modifiers.map(modifier => modifierLabel[modifier]), keyLabel].join('+')
}

/**
 * Turn a keydown in the recorder into an accelerator.
 * Returns { pending: true } while only modifiers are held, { cancel: true }
 * for a bare Escape, { error } for a key that is refused, or { accelerator }.
 */
export function acceleratorFromKeyEvent(event) {
  if (MODIFIER_KEYS.has(event.key)) return { pending: true }
  const code = event.code
  const modifiers = []
  if (event.ctrlKey) modifiers.push('Control')
  if (event.altKey) modifiers.push('Alt')
  if (event.shiftKey) modifiers.push('Shift')
  if (event.metaKey) modifiers.push('Super')
  if (code === 'Escape' || event.key === 'Escape') {
    return modifiers.length === 0 ? { cancel: true } : { error: playbackKeyProblem('Escape') }
  }
  if (!code || !isSupportedKeyCode(code) || REFUSED_KEYS.has(code)) {
    return { error: 'That key can’t be used as a playback key. Choose a function key, or a key with Ctrl or Cmd.' }
  }
  const accelerator = [...modifiers, code].join('+')
  const problem = playbackKeyProblem(accelerator)
  return problem ? { error: problem } : { accelerator }
}

/** Which action already uses this accelerator, other than `exceptAction`. */
export function actionUsingKey(bindings, accelerator, exceptAction) {
  const wanted = parseAccelerator(accelerator)
  if (!wanted) return null
  const same = (a, b) => a.key === b.key && a.modifiers.join('+') === b.modifiers.join('+')
  for (const [action, value] of Object.entries(bindings || {})) {
    if (action === exceptAction) continue
    const parsed = parseAccelerator(value)
    if (parsed && same(parsed, wanted)) return action
  }
  return null
}

/**
 * Settings as stored, cleaned: unknown actions, refused keys and duplicate
 * keys are dropped, so a hand-edited preferences file cannot register a bare
 * letter. `enabled` stays false unless it is exactly true.
 */
export function normalizePlaybackKeys(raw) {
  const source = raw && typeof raw === 'object' ? raw : {}
  const bindings = {}
  const rawBindings = source.bindings && typeof source.bindings === 'object' ? source.bindings : {}
  for (const { id } of PLAYBACK_KEY_ACTIONS) {
    const value = rawBindings[id]
    if (typeof value !== 'string' || playbackKeyProblem(value)) continue
    if (actionUsingKey(bindings, value, id)) continue
    const parsed = parseAccelerator(value)
    bindings[id] = [...parsed.modifiers, parsed.key].join('+')
  }
  return { enabled: source.enabled === true, holdToPlay: source.holdToPlay === true, bindings }
}

export function backAmount(backstep) {
  return Number.isFinite(backstep) && backstep > 0 ? backstep : BACK_FALLBACK_SECONDS
}

export const NO_PLAYER_MESSAGE = 'Key pressed, but the Player is not open.'
export const NO_TRACK_MESSAGE = 'Key pressed, but no recording is loaded.'

// A press with no release in between is a held key or an auto-repeat. After
// this long, a missing release is treated as lost, so the key is never stuck.
const HELD_KEY_TIMEOUT_MS = 10_000

/**
 * Turn plugin events into Player actions.
 *
 * setTarget(target) gives it the Player's actions, or null when the Player is
 * not open: { hasTrack, isPlaying(), play(), pause(), toggle(), seekBy(s),
 * cycleSpeed(dir), backstep }. onStatus(message) receives '' after an action
 * ran, or the reason nothing happened.
 */
export function createPlaybackKeyDispatcher({ onStatus = () => {}, now = () => Date.now() } = {}) {
  const pressedAt = new Map()
  let target = null
  let holdToPlay = false

  function handle(action, state) {
    if (!ACTION_IDS.has(action)) return
    if (state === 'Released') {
      const wasHeld = pressedAt.has(action)
      pressedAt.delete(action)
      if (action !== 'playPause' || !holdToPlay || !wasHeld) return
      if (target?.hasTrack && target.isPlaying()) target.pause()
      return
    }
    if (state !== 'Pressed') return
    const since = pressedAt.get(action)
    if (since != null && now() - since < HELD_KEY_TIMEOUT_MS) return
    pressedAt.set(action, now())

    if (!target) {
      onStatus(NO_PLAYER_MESSAGE)
      return
    }
    if (!target.hasTrack) {
      onStatus(NO_TRACK_MESSAGE)
      return
    }
    onStatus('')
    switch (action) {
      case 'playPause':
        if (holdToPlay) {
          if (!target.isPlaying()) target.play()
        } else {
          target.toggle()
        }
        break
      case 'back':
        target.seekBy(-backAmount(target.backstep))
        break
      case 'forward':
        target.seekBy(FORWARD_SECONDS)
        break
      case 'slower':
        target.cycleSpeed(-1)
        break
      case 'faster':
        target.cycleSpeed(1)
        break
      default:
        break
    }
  }

  return {
    handle,
    setTarget(next) {
      target = next || null
    },
    setHoldToPlay(next) {
      holdToPlay = next === true
    },
  }
}
