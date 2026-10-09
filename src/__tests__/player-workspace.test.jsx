import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { open } from '@tauri-apps/plugin-dialog'
import App from '../App'
import PlayerTab from '../components/Player/PlayerTab'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
  invoke: vi.fn(command => Promise.resolve(command === 'library_get' ? [] : { ffmpeg: true })),
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))
vi.mock('../hooks/PreferencesContext', () => ({
  usePreferencesContext: () => ({ labels: [], outDir: '', prefsReady: true, prefsError: '', themePref: 'dark' }),
}))
vi.mock('../hooks/useTheme', () => ({ default: () => ({ themeLabel: 'dark', themePref: 'dark' }) }))
vi.mock('../hooks/useFileDrop', () => ({ default: () => ({ files: [], caseName: '' }) }))
vi.mock('../hooks/useConversion', () => ({ default: () => ({ jobs: {}, converting: false }) }))
vi.mock('../hooks/useUpdater', () => ({ default: () => ({ status: 'idle' }) }))
vi.mock('../components/Convert/ConvertTab', () => ({
  default: ({ onOpenInPlayer }) => (
    <button onClick={() => onOpenInPlayer(['/recordings/output.wav'])}>Review converted output</button>
  ),
}))
vi.mock('../components/Library/LibraryTab', () => ({ default: () => <p>Library workspace</p> }))
vi.mock('../components/SettingsPanel', () => ({ default: () => null }))
vi.mock('../components/common/Waveform', () => ({ default: () => <div /> }))
vi.mock('../components/Player/Transcript', () => ({ default: () => <div /> }))
vi.mock('../lib/speakerColors', () => ({
  SPEAKER_COUNT: 4,
  useSpeakerColors: () => ['#111111'],
  speakerColorAt: () => '#111111',
}))

const switchWorkspace = name => fireEvent.mouseDown(screen.getByRole('tab', { name }), { button: 0, ctrlKey: false })
const addAudio = async () => {
  fireEvent.click(await screen.findByRole('button', { name: /add audio files to the playlist/i }))
  await screen.findByRole('textbox', { name: 'Track label for hearing.wav' })
}

describe('Player workspace continuity and controls', () => {
  beforeEach(() => {
    window.localStorage.clear()
    vi.spyOn(window.HTMLMediaElement.prototype, 'load').mockImplementation(() => {})
    vi.spyOn(window.HTMLMediaElement.prototype, 'pause').mockImplementation(() => {})
    vi.spyOn(window.HTMLMediaElement.prototype, 'play').mockResolvedValue(undefined)
    open.mockResolvedValue(['/recordings/hearing.wav', '/recordings/backup.wav'])
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    vi.restoreAllMocks()
  })

  it('preserves the playlist, labels, position and loop when switching workspaces', async () => {
    const { container } = render(<App />)
    switchWorkspace('Player')
    await addAudio()
    fireEvent.change(screen.getByRole('textbox', { name: 'Track label for hearing.wav' }), {
      target: { value: 'Reporter microphone' },
    })
    const audio = container.querySelector('audio')
    Object.defineProperty(audio, 'duration', { configurable: true, value: 180 })
    fireEvent.loadedMetadata(audio)
    audio.currentTime = 30
    fireEvent.timeUpdate(audio)
    fireEvent.click(screen.getByTitle('Set loop start'))
    audio.currentTime = 45
    fireEvent.timeUpdate(audio)
    fireEvent.click(screen.getByTitle('Set loop end'))
    switchWorkspace('Library')
    await screen.findByText('Library workspace')
    expect(screen.queryByRole('textbox', { name: 'Track label for hearing.wav' })).not.toBeInTheDocument()
    expect(audio.pause).toHaveBeenCalled()
    switchWorkspace('Player')
    expect(await screen.findByRole('textbox', { name: 'Track label for hearing.wav' })).toHaveValue(
      'Reporter microphone',
    )
    expect(container.querySelector('audio')).toBe(audio)
    expect(audio.currentTime).toBe(45)
    expect(screen.getByTitle('Set loop start')).toHaveTextContent('0:30')
    expect(screen.getByTitle('Set loop end')).toHaveTextContent('0:45')
  })

  it('does not claim drops or transport keys while inactive', async () => {
    const dropHandlerRef = { current: null }
    const { container, rerender } = render(<PlayerTab active dropHandlerRef={dropHandlerRef} />)
    await addAudio()
    expect(dropHandlerRef.current).toBeTypeOf('function')
    const audio = container.querySelector('audio')
    rerender(<PlayerTab active={false} dropHandlerRef={dropHandlerRef} />)
    expect(dropHandlerRef.current).toBeNull()
    audio.play.mockClear()
    fireEvent.keyDown(window, { key: 'k' })
    expect(audio.play).not.toHaveBeenCalled()
    rerender(<PlayerTab active dropHandlerRef={dropHandlerRef} />)
    expect(dropHandlerRef.current).toBeTypeOf('function')
  })

  it('seeks only once and keeps speed unchanged when a scrubber handles arrow keys', async () => {
    const { container } = render(<PlayerTab />)
    await addAudio()
    const audio = container.querySelector('audio')
    Object.defineProperty(audio, 'duration', { configurable: true, value: 180 })
    audio.currentTime = 30
    fireEvent.loadedMetadata(audio)
    fireEvent.timeUpdate(audio)
    const seek = screen.getByRole('slider', { name: 'Seek' })
    fireEvent.keyDown(seek, { key: 'ArrowRight' })
    expect(audio.currentTime).toBe(35)
    fireEvent.keyDown(seek, { key: 'ArrowUp' })
    expect(audio.currentTime).toBe(40)
    expect(audio.playbackRate).toBe(1)
  })

  it('offers compact speed selection and keyboard-operable reorder actions', async () => {
    render(<PlayerTab />)
    await addAudio()
    const speed = screen.getByRole('combobox', { name: 'Playback speed' })
    fireEvent.change(speed, { target: { value: '0.75' } })
    expect(speed).toHaveValue('0.75')
    expect(window.localStorage.getItem('player-speed')).toBe('0.75')
    fireEvent.click(screen.getByRole('button', { name: 'Move backup.wav up' }))
    expect(
      screen.getAllByRole('textbox', { name: /Track label for/ }).map(input => input.getAttribute('aria-label')),
    ).toEqual(['Track label for backup.wav', 'Track label for hearing.wav'])
    expect(screen.getByRole('button', { name: 'Move backup.wav up' })).toBeDisabled()
    expect(screen.getByRole('status', { name: 'Playlist order' })).toHaveTextContent(
      'backup.wav moved to position 1 of 2',
    )
  })

  it('opens a converted output before Player has ever been visited', async () => {
    render(<App />)
    fireEvent.click(screen.getByRole('button', { name: 'Review converted output' }))
    expect(await screen.findByRole('textbox', { name: 'Track label for output.wav' })).toHaveValue('Track 1')
    await waitFor(() => expect(screen.getByRole('tab', { name: 'Player' })).toHaveAttribute('aria-selected', 'true'))
    switchWorkspace('Convert')
    fireEvent.click(screen.getByRole('button', { name: 'Review converted output' }))
    expect(await screen.findByRole('textbox', { name: 'Track label for output.wav' })).toHaveValue('Track 1')
    expect(screen.getAllByRole('textbox', { name: /Track label for/ })).toHaveLength(1)
  })
})
