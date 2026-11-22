import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { register, unregisterAll } from '@tauri-apps/plugin-global-shortcut'
import usePlaybackKeys from '../hooks/usePlaybackKeys'
import PlaybackKeysSettings from '../components/PlaybackKeysSettings'
import { normalizePlaybackKeys, registrationErrorText } from '../lib/playbackKeys'

vi.mock('@tauri-apps/plugin-global-shortcut', () => ({ register: vi.fn(), unregisterAll: vi.fn() }))

beforeEach(() => {
  register.mockReset()
  unregisterAll.mockReset()
  register.mockResolvedValue(undefined)
  unregisterAll.mockResolvedValue(undefined)
})
afterEach(cleanup)

describe('usePlaybackKeys registration', () => {
  it('registers nothing while off, even with keys assigned', async () => {
    const { result } = renderHook(() => usePlaybackKeys({ enabled: false, bindings: { playPause: 'F14' } }))
    await waitFor(() => expect(unregisterAll).toHaveBeenCalled())
    expect(register).not.toHaveBeenCalled()
    expect(result.current.registration).toEqual({})
  })

  it('registers each key on its own and reports a refused key per key', async () => {
    register.mockImplementation(accelerator =>
      accelerator === 'F13' ? Promise.reject(new Error('HotKey already registered')) : Promise.resolve(),
    )
    const settings = { enabled: true, bindings: { back: 'F13', playPause: 'F14' } }
    const { result } = renderHook(() => usePlaybackKeys(settings))
    await waitFor(() => expect(result.current.registration.playPause?.state).toBe('registered'))
    expect(result.current.registration.back).toMatchObject({ state: 'failed', accelerator: 'F13' })
    expect(result.current.registration.back.error).toMatch(/already registered/)
    expect(register).toHaveBeenCalledTimes(2)
  })

  it('never registers a bare letter or Space from stored settings', async () => {
    const settings = { enabled: true, bindings: { back: 'KeyB', playPause: 'Space', forward: 'F15' } }
    renderHook(() => usePlaybackKeys(settings))
    await waitFor(() => expect(register).toHaveBeenCalledTimes(1))
    expect(register.mock.calls[0][0]).toBe('F15')
  })

  it('forgets a held key when keys are released, so the next press after a re-sync works', async () => {
    const settings = { enabled: true, bindings: { forward: 'F15' } }
    const { result } = renderHook(() => usePlaybackKeys(settings))
    await waitFor(() => expect(register).toHaveBeenCalledTimes(1))
    const seekBy = vi.fn()
    act(() =>
      result.current.setTarget({ hasTrack: true, isPlaying: () => false, seekBy, play() {}, pause() {}, toggle() {} }),
    )
    // Pressed, then the recorder suspends keys; the Released is never delivered
    act(() => result.current.dispatch('forward', 'Pressed'))
    act(() => result.current.setSuspended(true))
    await waitFor(() => expect(unregisterAll).toHaveBeenCalledTimes(2))
    act(() => result.current.setSuspended(false))
    await waitFor(() => expect(register).toHaveBeenCalledTimes(2))
    act(() => result.current.dispatch('forward', 'Pressed'))
    expect(seekBy).toHaveBeenCalledTimes(2)
  })

  it('releases every key when turned off, while recording, and on unmount', async () => {
    let settings = { enabled: true, bindings: { playPause: 'F14' } }
    const { result, rerender, unmount } = renderHook(() => usePlaybackKeys(settings))
    await waitFor(() => expect(register).toHaveBeenCalledTimes(1))

    act(() => result.current.setSuspended(true))
    await waitFor(() => expect(result.current.registration).toEqual({}))
    expect(register).toHaveBeenCalledTimes(1)

    act(() => result.current.setSuspended(false))
    await waitFor(() => expect(register).toHaveBeenCalledTimes(2))

    settings = { enabled: false, bindings: { playPause: 'F14' } }
    rerender()
    await waitFor(() => expect(result.current.registration).toEqual({}))
    const releasesBeforeUnmount = unregisterAll.mock.calls.length
    unmount()
    await waitFor(() => expect(unregisterAll.mock.calls.length).toBeGreaterThan(releasesBeforeUnmount))
    expect(register).toHaveBeenCalledTimes(2)
  })

  it('sends plugin press and release events to the Player through the same actions', async () => {
    const settings = { enabled: true, holdToPlay: true, bindings: { playPause: 'F14' } }
    const { result } = renderHook(() => usePlaybackKeys(settings))
    await waitFor(() => expect(register).toHaveBeenCalledTimes(1))
    const handler = register.mock.calls[0][1]

    act(() => handler({ shortcut: 'F14', state: 'Pressed' }))
    expect(result.current.status).toMatch(/Player is not open/)
    act(() => handler({ shortcut: 'F14', state: 'Released' }))

    let playing = false
    const target = {
      hasTrack: true,
      backstep: 0,
      isPlaying: () => playing,
      play: vi.fn(() => (playing = true)),
      pause: vi.fn(() => (playing = false)),
      toggle: vi.fn(),
      seekBy: vi.fn(),
      cycleSpeed: vi.fn(),
    }
    act(() => result.current.setTarget(target))
    expect(result.current.status).toBe('')
    act(() => handler({ shortcut: 'F14', state: 'Pressed' }))
    expect(target.play).toHaveBeenCalledTimes(1)
    act(() => handler({ shortcut: 'F14', state: 'Released' }))
    expect(target.pause).toHaveBeenCalledTimes(1)
    expect(target.toggle).not.toHaveBeenCalled()
  })
})

function Harness({ initial = {} }) {
  const [playbackKeys, setPlaybackKeysState] = useState(normalizePlaybackKeys(initial))
  const setPlaybackKeys = next => setPlaybackKeysState(normalizePlaybackKeys(next))
  const keys = usePlaybackKeys(playbackKeys)
  return <PlaybackKeysSettings prefs={{ playbackKeys, setPlaybackKeys }} keys={keys} />
}

describe('Playback keys settings section', () => {
  it('is off by default, warns about taking keys from every app, and registers nothing', async () => {
    render(<Harness />)
    expect(screen.getByRole('switch', { name: /Control the Player from other apps/ })).toHaveAttribute(
      'aria-checked',
      'false',
    )
    expect(screen.getByText(/takes these keys from every app/)).toBeInTheDocument()
    expect(screen.getByText('Off. No keys are taken from other apps.')).toBeInTheDocument()
    expect(screen.getByText('Suggested: F14')).toBeInTheDocument()
    await waitFor(() => expect(unregisterAll).toHaveBeenCalled())
    expect(register).not.toHaveBeenCalled()
  })

  it('copies suggestions only when asked, and registers them only after turning on', async () => {
    render(<Harness />)
    fireEvent.click(screen.getByRole('button', { name: 'Use suggested keys' }))
    expect(screen.getAllByText('Not registered (off)')).toHaveLength(3)
    await waitFor(() => expect(unregisterAll).toHaveBeenCalled())
    expect(register).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('switch', { name: /Control the Player from other apps/ }))
    await waitFor(() => expect(screen.getAllByText('Registered')).toHaveLength(3))
    expect(register.mock.calls.map(([accelerator]) => accelerator)).toEqual(['F13', 'F14', 'F15'])
    expect(screen.getByText(/On\. 3 keys work in every app/)).toBeInTheDocument()
  })

  it('records a key, refuses bare letters and Space, and pauses keys while listening', async () => {
    render(<Harness initial={{ enabled: true, bindings: { back: 'F13' } }} />)
    await waitFor(() => expect(register).toHaveBeenCalledTimes(1))

    const assign = screen.getByRole('button', { name: 'Assign a key to Play/Pause' })
    fireEvent.click(assign)
    const listening = screen.getByRole('button', { name: /Press a key for Play\/Pause/ })
    await waitFor(() => expect(screen.getByText('Keys are released while you assign a key.')).toBeInTheDocument())

    fireEvent.keyDown(listening, { key: 'k', code: 'KeyK' })
    expect(screen.getByRole('alert')).toHaveTextContent(/Add Ctrl or Cmd/)
    fireEvent.keyDown(listening, { key: ' ', code: 'Space' })
    expect(screen.getByRole('alert')).toHaveTextContent(/Space/)
    fireEvent.keyDown(listening, { key: 'F13', code: 'F13' })
    expect(screen.getByRole('alert')).toHaveTextContent(/already assigned to Back/)

    fireEvent.keyDown(listening, { key: 'p', code: 'KeyP', ctrlKey: true, altKey: true })
    expect(screen.getByRole('alert')).toHaveTextContent(/AltGr/)
    fireEvent.keyDown(listening, { key: 'ArrowLeft', code: 'ArrowLeft', ctrlKey: true })
    expect(screen.getByRole('alert')).toHaveTextContent(/edits text/)
    fireEvent.keyDown(listening, { key: 'K', code: 'KeyK', ctrlKey: true, shiftKey: true })
    await waitFor(() => expect(register.mock.calls.map(([accelerator]) => accelerator)).toContain('Control+Shift+KeyK'))
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('cancels recording with Escape and keeps the previous key', async () => {
    render(<Harness initial={{ enabled: false, bindings: { playPause: 'F14' } }} />)
    fireEvent.click(screen.getByRole('button', { name: 'Change the key for Play/Pause' }))
    fireEvent.keyDown(screen.getByRole('button', { name: /Press a key for Play\/Pause/ }), {
      key: 'Escape',
      code: 'Escape',
    })
    // The accessible name starts with the visible word (WCAG 2.5.3 Label in Name)
    const change = screen.getByRole('button', { name: 'Change the key for Play/Pause' })
    expect(change).toHaveTextContent('Change')
    expect(change.getAttribute('aria-label')).toMatch(/^Change/)
    expect(screen.getByText('F14')).toBeInTheDocument()
  })

  it('shows a key the system refused next to that key', async () => {
    const raw = 'HotKey already registered: HotKey { mods: Modifiers(0x0), key: F15, id: 2785 }'
    register.mockImplementation(accelerator => (accelerator === 'F15' ? Promise.reject(raw) : Promise.resolve()))
    render(<Harness initial={{ enabled: true, bindings: { back: 'F13', forward: 'F15' } }} />)
    expect(await screen.findByText('Another app is already using this key. Choose another key.')).toBeInTheDocument()
    // The plugin's own text is kept, but only behind "System message"
    expect(screen.getByText('System message')).toBeInTheDocument()
    expect(screen.getByText(raw).closest('details')).not.toBeNull()
    expect(screen.getByText('Registered')).toBeInTheDocument()
    expect(screen.getByText(/1 of 2 keys could not be registered/)).toBeInTheDocument()
  })

  it('maps plugin errors to plain words', () => {
    expect(registrationErrorText('Unable to register hotkey: Access is denied. (os error 1409)')).toBe(
      'The system refused this key. Choose another key.',
    )
    expect(registrationErrorText('something else')).toBe('The system couldn’t register this key. Choose another key.')
  })

  it('does not claim pedals can send F13–F15 and warns that Hold to play needs key releases', () => {
    render(<Harness />)
    expect(screen.getByText(/may be able to send them if it offers F13–F15/)).toBeInTheDocument()
    expect(screen.getByText(/needs the pedal or helper app to send a key release/)).toBeInTheDocument()
    expect(screen.queryByText(/Pedable/)).not.toBeInTheDocument()
  })

  it('explains hold to play with the Player Backstep', () => {
    render(<Harness />)
    fireEvent.click(screen.getByRole('tab', { name: 'Hold to play' }))
    expect(screen.getByText(/plays while the Play\/Pause key or pedal is held down/)).toBeInTheDocument()
    expect(screen.getByText(/starts where it paused \(the Player’s Backstep is Off\)/)).toBeInTheDocument()
  })
})
