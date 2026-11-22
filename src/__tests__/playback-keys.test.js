import { describe, expect, it, vi } from 'vitest'
import {
  acceleratorFromKeyEvent,
  actionUsingKey,
  backAmount,
  createPlaybackKeyDispatcher,
  formatPlaybackKey,
  normalizePlaybackKeys,
  playbackKeyNote,
  playbackKeyProblem,
  playbackKeysSidebarLabel,
  NO_PLAYER_MESSAGE,
  NO_TRACK_MESSAGE,
  SUGGESTED_PLAYBACK_KEYS,
} from '../lib/playbackKeys'

function key(code, keyName = code, modifiers = {}) {
  return { code, key: keyName, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...modifiers }
}

describe('key recorder validation', () => {
  it('refuses bare letters, digits and Space', () => {
    expect(acceleratorFromKeyEvent(key('KeyK', 'k')).error).toMatch(/Add Ctrl or Cmd/)
    expect(acceleratorFromKeyEvent(key('Digit1', '1')).error).toMatch(/Add Ctrl or Cmd/)
    expect(acceleratorFromKeyEvent(key('Space', ' ')).error).toMatch(/Space/)
    expect(acceleratorFromKeyEvent(key('Numpad5', '5')).error).toBeTruthy()
  })

  it('refuses Shift or Alt alone with a typing key, since those still type characters', () => {
    expect(acceleratorFromKeyEvent(key('KeyP', 'P', { shiftKey: true })).error).toBeTruthy()
    expect(acceleratorFromKeyEvent(key('KeyP', 'π', { altKey: true })).error).toBeTruthy()
    expect(acceleratorFromKeyEvent(key('Space', ' ', { shiftKey: true })).error).toBeTruthy()
  })

  it('cancels on a bare Escape and refuses Escape with modifiers', () => {
    expect(acceleratorFromKeyEvent(key('Escape'))).toEqual({ cancel: true })
    expect(acceleratorFromKeyEvent(key('Escape', 'Escape', { ctrlKey: true })).error).toMatch(/Escape/)
    expect(playbackKeyProblem('Escape')).toMatch(/Escape/)
    expect(playbackKeyProblem('Control+Escape')).toMatch(/Escape/)
  })

  it('waits while only modifiers are held', () => {
    expect(acceleratorFromKeyEvent(key('ControlLeft', 'Control', { ctrlKey: true }))).toEqual({ pending: true })
    expect(acceleratorFromKeyEvent(key('MetaLeft', 'Meta', { metaKey: true }))).toEqual({ pending: true })
  })

  it('accepts function keys bare or with modifiers', () => {
    expect(acceleratorFromKeyEvent(key('F14'))).toEqual({ accelerator: 'F14' })
    expect(acceleratorFromKeyEvent(key('F9', 'F9', { shiftKey: true }))).toEqual({ accelerator: 'Shift+F9' })
    expect(acceleratorFromKeyEvent(key('F24'))).toEqual({ accelerator: 'F24' })
  })

  it('accepts a key with Ctrl or Cmd, in the plugin modifier order', () => {
    expect(acceleratorFromKeyEvent(key('KeyK', 'K', { ctrlKey: true, shiftKey: true }))).toEqual({
      accelerator: 'Control+Shift+KeyK',
    })
    expect(acceleratorFromKeyEvent(key('KeyP', 'π', { metaKey: true, altKey: true }), { mac: true })).toEqual({
      accelerator: 'Alt+Super+KeyP',
    })
    expect(acceleratorFromKeyEvent(key('Digit7', '7', { ctrlKey: true }))).toEqual({ accelerator: 'Control+Digit7' })
  })

  it('refuses copy, paste, undo, redo and save, whatever other modifiers are held', () => {
    for (const code of ['KeyC', 'KeyV', 'KeyX', 'KeyZ', 'KeyY', 'KeyA', 'KeyS']) {
      const letter = code.slice(3).toLowerCase()
      expect(acceleratorFromKeyEvent(key(code, letter, { ctrlKey: true })).error).toMatch(/copy, paste/)
      expect(acceleratorFromKeyEvent(key(code, letter, { ctrlKey: true, shiftKey: true })).error).toMatch(/copy, paste/)
      expect(acceleratorFromKeyEvent(key(code, letter, { metaKey: true, shiftKey: true })).error).toMatch(/copy, paste/)
    }
    expect(playbackKeyProblem('Control+Shift+KeyZ')).toBeTruthy()
    expect(playbackKeyProblem('Shift+Super+KeyZ')).toBeTruthy()
    expect(playbackKeyProblem('Control+Shift+KeyV')).toBeTruthy()
  })

  it('refuses Ctrl+Alt without Cmd/Win, which is AltGr on Windows and types characters', () => {
    for (const accelerator of [
      'Control+Alt+KeyQ',
      'Control+Alt+KeyE',
      'Control+Alt+Digit7',
      'Control+Alt+KeyA',
      'Control+Alt+Shift+KeyP',
      'Control+Alt+Period',
    ]) {
      expect(playbackKeyProblem(accelerator)).toMatch(/AltGr/)
    }
    expect(acceleratorFromKeyEvent(key('KeyP', 'p', { ctrlKey: true, altKey: true })).error).toMatch(/AltGr/)
    expect(playbackKeyProblem('Control+Alt+F13')).toBe('')
  })

  it('refuses editing, cursor, Space and Tab keys with Ctrl or Cmd', () => {
    for (const named of [
      'Backspace',
      'Delete',
      'Insert',
      'ArrowLeft',
      'ArrowRight',
      'ArrowUp',
      'ArrowDown',
      'Home',
      'End',
      'PageUp',
      'PageDown',
      'Enter',
      'Tab',
      'Space',
    ]) {
      expect(playbackKeyProblem(`Control+${named}`)).toMatch(/edits text/)
      expect(playbackKeyProblem(`Super+${named}`)).toMatch(/edits text/)
      expect(playbackKeyProblem(`Control+Shift+${named}`)).toMatch(/edits text/)
    }
    expect(acceleratorFromKeyEvent(key('ArrowLeft', 'ArrowLeft', { ctrlKey: true })).error).toMatch(/edits text/)
  })

  it('refuses window-closing and quitting keys', () => {
    expect(playbackKeyProblem('Alt+F4')).toMatch(/close/)
    expect(playbackKeyProblem('Control+F4')).toMatch(/close/)
    expect(playbackKeyProblem('Control+Shift+F4')).toMatch(/close/)
    expect(playbackKeyProblem('F4')).toBe('')
    expect(playbackKeyProblem('Super+KeyQ')).toMatch(/quits/)
    expect(playbackKeyProblem('Super+KeyW')).toMatch(/quits/)
    expect(playbackKeyProblem('Shift+Super+KeyQ')).toMatch(/quits/)
  })

  it('notes formatting and document keys without refusing them', () => {
    for (const letter of ['B', 'I', 'U', 'F', 'P', 'N', 'O']) {
      expect(playbackKeyProblem(`Control+Key${letter}`)).toBe('')
      expect(playbackKeyNote(`Control+Key${letter}`)).toMatch(/word processors and CAT editors/)
      expect(playbackKeyNote(`Super+Key${letter}`)).toMatch(/word processors and CAT editors/)
    }
    expect(playbackKeyNote('Control+KeyW')).toMatch(/close/)
    expect(playbackKeyNote('Control+Shift+KeyB')).toBe('')
    expect(playbackKeyNote('Control+KeyK')).toBe('')
  })

  it('stores letters by the layout letter on Windows and Linux, and by position on macOS', () => {
    // AZERTY: the key labelled A is in the QWERTY Q position
    expect(acceleratorFromKeyEvent(key('KeyQ', 'a', { ctrlKey: true }), { mac: false })).toEqual({
      error: expect.stringMatching(/copy, paste/),
    })
    expect(acceleratorFromKeyEvent(key('KeyW', 'z', { ctrlKey: true, shiftKey: true }), { mac: false }).error).toMatch(
      /copy, paste/,
    )
    expect(acceleratorFromKeyEvent(key('KeyA', 'q', { ctrlKey: true }), { mac: false })).toEqual({
      accelerator: 'Control+KeyQ',
    })
    expect(acceleratorFromKeyEvent(key('KeyM', 'q', { metaKey: true }), { mac: true })).toEqual({
      accelerator: 'Super+KeyM',
    })
    // A non-Latin layout keeps the position, which matches the Latin key code
    expect(acceleratorFromKeyEvent(key('KeyK', 'л', { ctrlKey: true }), { mac: false })).toEqual({
      accelerator: 'Control+KeyK',
    })
  })

  it('refuses punctuation that gives a different character on this layout on Windows and Linux', () => {
    expect(acceleratorFromKeyEvent(key('Comma', ',', { ctrlKey: true }), { mac: false })).toEqual({
      accelerator: 'Control+Comma',
    })
    expect(acceleratorFromKeyEvent(key('Semicolon', 'm', { ctrlKey: true }), { mac: false }).error).toMatch(
      /keyboard layout/,
    )
    expect(acceleratorFromKeyEvent(key('Semicolon', 'm', { ctrlKey: true }), { mac: true })).toEqual({
      accelerator: 'Control+Semicolon',
    })
  })

  it('refuses lock, screenshot, media and unknown keys', () => {
    for (const code of ['CapsLock', 'NumLock', 'ScrollLock', 'PrintScreen', 'MediaPlayPause', 'AudioVolumeUp', '']) {
      expect(acceleratorFromKeyEvent(key(code, 'x', { ctrlKey: true })).error).toBeTruthy()
    }
  })

  it('finds a key already assigned to another action, whatever the modifier order', () => {
    const bindings = { back: 'F13', playPause: 'Control+Shift+KeyK' }
    expect(actionUsingKey(bindings, 'F13', 'forward')).toBe('back')
    expect(actionUsingKey(bindings, 'Shift+Control+KeyK', 'forward')).toBe('playPause')
    expect(actionUsingKey(bindings, 'F13', 'back')).toBeNull()
  })

  it('notes known default uses in other transcription tools without refusing them', () => {
    expect(playbackKeyNote('Shift+F9')).toMatch(/CATalyst/)
    expect(playbackKeyNote('F9')).toMatch(/Express Scribe/)
    expect(playbackKeyProblem('F9')).toBe('')
    expect(playbackKeyNote('F14')).toBe('')
  })

  it('suggests keys that pass validation and have no known collision', () => {
    for (const accelerator of Object.values(SUGGESTED_PLAYBACK_KEYS)) {
      expect(playbackKeyProblem(accelerator)).toBe('')
      expect(playbackKeyNote(accelerator)).toBe('')
    }
  })

  it('formats keys for each platform', () => {
    expect(formatPlaybackKey('Control+Shift+KeyP', { mac: false })).toBe('Ctrl+Shift+P')
    expect(formatPlaybackKey('Alt+Super+Digit2', { mac: true })).toBe('Option+Cmd+2')
    expect(formatPlaybackKey('Control+Numpad4', { mac: false })).toBe('Ctrl+Num 4')
    expect(formatPlaybackKey('F14')).toBe('F14')
  })
})

describe('stored settings', () => {
  it('defaults off with no keys, including for missing or malformed data', () => {
    for (const raw of [undefined, null, 'yes', {}, { enabled: 'true' }, { enabled: 1 }]) {
      expect(normalizePlaybackKeys(raw)).toEqual({ enabled: false, holdToPlay: false, bindings: {} })
    }
  })

  it('drops refused keys, unknown actions and duplicates from a hand-edited file', () => {
    const result = normalizePlaybackKeys({
      enabled: true,
      holdToPlay: true,
      bindings: {
        back: 'KeyB',
        playPause: 'Space',
        forward: 'F15',
        slower: 'F15',
        faster: 'Shift+Control+Period',
        launch: 'F1',
      },
    })
    expect(result).toEqual({
      enabled: true,
      holdToPlay: true,
      bindings: { forward: 'F15', faster: 'Control+Shift+Period' },
    })
  })

  it('drops keys a hand-edited file could use to take typing keys', () => {
    const result = normalizePlaybackKeys({
      enabled: true,
      bindings: {
        back: 'Control+ArrowLeft',
        playPause: 'Control+Alt+KeyQ',
        forward: 'Control+Shift+KeyZ',
        slower: 'Alt+F4',
        faster: 'Super+Space',
      },
    })
    expect(result.bindings).toEqual({})
  })
})

function fakePlayer(overrides = {}) {
  let playing = false
  const player = {
    hasTrack: true,
    backstep: 0,
    isPlaying: () => playing,
    play: vi.fn(() => {
      playing = true
    }),
    pause: vi.fn(() => {
      playing = false
    }),
    toggle: vi.fn(() => {
      playing = !playing
    }),
    seekBy: vi.fn(),
    cycleSpeed: vi.fn(),
    ...overrides,
  }
  return player
}

describe('action mapping', () => {
  it('maps each action to the Player', () => {
    const player = fakePlayer({ backstep: 3 })
    const dispatcher = createPlaybackKeyDispatcher()
    dispatcher.setTarget(player)
    const press = action => {
      dispatcher.handle(action, 'Pressed')
      dispatcher.handle(action, 'Released')
    }
    press('playPause')
    expect(player.toggle).toHaveBeenCalledTimes(1)
    press('back')
    expect(player.seekBy).toHaveBeenLastCalledWith(-3)
    press('forward')
    expect(player.seekBy).toHaveBeenLastCalledWith(2)
    press('slower')
    expect(player.cycleSpeed).toHaveBeenLastCalledWith(-1)
    press('faster')
    expect(player.cycleSpeed).toHaveBeenLastCalledWith(1)
  })

  it('backs up 2 s when the Player Backstep is Off', () => {
    expect(backAmount(0)).toBe(2)
    expect(backAmount(undefined)).toBe(2)
    expect(backAmount(5)).toBe(5)
    const player = fakePlayer({ backstep: 0 })
    const dispatcher = createPlaybackKeyDispatcher()
    dispatcher.setTarget(player)
    dispatcher.handle('back', 'Pressed')
    expect(player.seekBy).toHaveBeenCalledWith(-2)
  })

  it('does nothing and says why when the Player is not open or has no recording', () => {
    const onStatus = vi.fn()
    const dispatcher = createPlaybackKeyDispatcher({ onStatus })
    dispatcher.handle('playPause', 'Pressed')
    expect(onStatus).toHaveBeenLastCalledWith(NO_PLAYER_MESSAGE)
    dispatcher.handle('playPause', 'Released')

    const player = fakePlayer({ hasTrack: false })
    dispatcher.setTarget(player)
    dispatcher.handle('playPause', 'Pressed')
    expect(onStatus).toHaveBeenLastCalledWith(NO_TRACK_MESSAGE)
    expect(player.toggle).not.toHaveBeenCalled()
  })

  it('ignores a repeated press until the key is released', () => {
    let clock = 0
    const player = fakePlayer()
    const dispatcher = createPlaybackKeyDispatcher({ now: () => clock })
    dispatcher.setTarget(player)
    dispatcher.handle('playPause', 'Pressed')
    clock += 500
    dispatcher.handle('playPause', 'Pressed')
    expect(player.toggle).toHaveBeenCalledTimes(1)
    dispatcher.handle('playPause', 'Released')
    dispatcher.handle('playPause', 'Pressed')
    expect(player.toggle).toHaveBeenCalledTimes(2)
  })

  it('treats a release that never arrived as lost after 10 s', () => {
    let clock = 0
    const player = fakePlayer()
    const dispatcher = createPlaybackKeyDispatcher({ now: () => clock })
    dispatcher.setTarget(player)
    dispatcher.handle('forward', 'Pressed')
    clock += 10_001
    dispatcher.handle('forward', 'Pressed')
    expect(player.seekBy).toHaveBeenCalledTimes(2)
  })

  it('forgets held keys on reset, so a release lost during a re-sync does not block the next press', () => {
    let clock = 0
    const player = fakePlayer()
    const dispatcher = createPlaybackKeyDispatcher({ now: () => clock })
    dispatcher.setTarget(player)
    dispatcher.handle('back', 'Pressed')
    dispatcher.reset()
    clock += 500
    dispatcher.handle('back', 'Pressed')
    expect(player.seekBy).toHaveBeenCalledTimes(2)
  })

  it('ignores unknown actions and states', () => {
    const player = fakePlayer()
    const dispatcher = createPlaybackKeyDispatcher()
    dispatcher.setTarget(player)
    dispatcher.handle('launch', 'Pressed')
    dispatcher.handle('playPause', undefined)
    expect(player.toggle).not.toHaveBeenCalled()
  })
})

describe('hold to play', () => {
  it('plays while held and pauses on release', () => {
    const player = fakePlayer()
    const dispatcher = createPlaybackKeyDispatcher()
    dispatcher.setHoldToPlay(true)
    dispatcher.setTarget(player)
    dispatcher.handle('playPause', 'Pressed')
    expect(player.play).toHaveBeenCalledTimes(1)
    expect(player.isPlaying()).toBe(true)
    dispatcher.handle('playPause', 'Released')
    expect(player.pause).toHaveBeenCalledTimes(1)
    expect(player.isPlaying()).toBe(false)
    expect(player.toggle).not.toHaveBeenCalled()
  })

  it('does not pause when the press was not seen, and never pauses on other keys', () => {
    const player = fakePlayer()
    const dispatcher = createPlaybackKeyDispatcher()
    dispatcher.setHoldToPlay(true)
    dispatcher.setTarget(player)
    player.toggle()
    dispatcher.handle('playPause', 'Released')
    dispatcher.handle('back', 'Pressed')
    dispatcher.handle('back', 'Released')
    expect(player.pause).not.toHaveBeenCalled()
    expect(player.isPlaying()).toBe(true)
  })

  it('leaves playback alone on release in press-to-toggle mode', () => {
    const player = fakePlayer()
    const dispatcher = createPlaybackKeyDispatcher()
    dispatcher.setTarget(player)
    dispatcher.handle('playPause', 'Pressed')
    dispatcher.handle('playPause', 'Released')
    expect(player.isPlaying()).toBe(true)
    expect(player.pause).not.toHaveBeenCalled()
  })
})

describe('sidebar label', () => {
  it('says the keys are on, or names keys the system refused', () => {
    expect(playbackKeysSidebarLabel({ registration: { back: { state: 'registered' } } })).toBe('Playback keys on')
    const oneFailed = { back: { state: 'registered' }, playPause: { state: 'failed', error: 'taken' } }
    expect(playbackKeysSidebarLabel({ registration: oneFailed })).toBe(
      'Playback keys on: 1 key not registered, see Settings',
    )
    const twoFailed = { ...oneFailed, forward: { state: 'failed' } }
    expect(playbackKeysSidebarLabel({ registration: twoFailed })).toBe(
      'Playback keys on: 2 keys not registered, see Settings',
    )
    expect(playbackKeysSidebarLabel({ syncError: 'Playback keys could not be released: x' })).toBe(
      'Playback keys on: keys could not be updated, see Settings',
    )
  })

  it('keeps a key-press status and the failure together', () => {
    expect(playbackKeysSidebarLabel({ status: NO_PLAYER_MESSAGE })).toBe(NO_PLAYER_MESSAGE)
    expect(playbackKeysSidebarLabel({ status: NO_PLAYER_MESSAGE, registration: { back: { state: 'failed' } } })).toBe(
      `Playback keys on: 1 key not registered, see Settings. ${NO_PLAYER_MESSAGE}`,
    )
  })
})
