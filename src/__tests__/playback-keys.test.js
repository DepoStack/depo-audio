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
    expect(acceleratorFromKeyEvent(key('KeyP', 'p', { ctrlKey: true, altKey: true }))).toEqual({
      accelerator: 'Control+Alt+KeyP',
    })
    expect(acceleratorFromKeyEvent(key('ArrowLeft', 'ArrowLeft', { metaKey: true, shiftKey: true }))).toEqual({
      accelerator: 'Shift+Super+ArrowLeft',
    })
  })

  it('refuses copy, paste, undo and save, but allows them with a second modifier', () => {
    for (const code of ['KeyC', 'KeyV', 'KeyX', 'KeyZ', 'KeyY', 'KeyA', 'KeyS']) {
      expect(acceleratorFromKeyEvent(key(code, code.slice(3).toLowerCase(), { ctrlKey: true })).error).toMatch(
        /copy, paste/,
      )
    }
    expect(acceleratorFromKeyEvent(key('KeyC', 'c', { ctrlKey: true, altKey: true }))).toEqual({
      accelerator: 'Control+Alt+KeyC',
    })
  })

  it('refuses lock, screenshot, media and unknown keys', () => {
    for (const code of ['CapsLock', 'NumLock', 'ScrollLock', 'PrintScreen', 'MediaPlayPause', 'AudioVolumeUp', '']) {
      expect(acceleratorFromKeyEvent(key(code, 'x', { ctrlKey: true })).error).toBeTruthy()
    }
  })

  it('finds a key already assigned to another action, whatever the modifier order', () => {
    const bindings = { back: 'F13', playPause: 'Control+Alt+KeyP' }
    expect(actionUsingKey(bindings, 'F13', 'forward')).toBe('back')
    expect(actionUsingKey(bindings, 'Alt+Control+KeyP', 'forward')).toBe('playPause')
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
    expect(formatPlaybackKey('Control+Alt+KeyP', { mac: false })).toBe('Ctrl+Alt+P')
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
        faster: 'Alt+Control+Period',
        launch: 'F1',
      },
    })
    expect(result).toEqual({
      enabled: true,
      holdToPlay: true,
      bindings: { forward: 'F15', faster: 'Control+Alt+Period' },
    })
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
