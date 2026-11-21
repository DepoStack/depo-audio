import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import PlayerTab from '../components/Player/PlayerTab'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}))

vi.mock('../components/common/Waveform', () => ({
  default: () => <div data-testid="waveform" />,
}))

vi.mock('../lib/speakerColors', () => ({
  SPEAKER_COUNT: 4,
  useSpeakerColors: () => ['#111111', '#222222', '#333333', '#444444'],
  speakerColorAt: (colors, index) => colors[index % colors.length],
}))

vi.mock('../components/Player/Transcript', () => ({
  default: () => <div data-testid="synced-transcript" />,
}))

// jsdom has no media pipeline: give the Player's <audio> a fake clock and
// fire the same play/pause/seeking events a real element would.
function fakeAudio(audio, { duration = 120 } = {}) {
  let time = 0
  let paused = true
  Object.defineProperty(audio, 'paused', { configurable: true, get: () => paused })
  Object.defineProperty(audio, 'ended', { configurable: true, get: () => false })
  Object.defineProperty(audio, 'duration', { configurable: true, get: () => duration })
  Object.defineProperty(audio, 'currentTime', {
    configurable: true,
    get: () => time,
    set: value => {
      time = value
      audio.dispatchEvent(new Event('seeking'))
    },
  })
  audio.play = vi.fn(() => {
    paused = false
    audio.dispatchEvent(new Event('play'))
    return Promise.resolve()
  })
  audio.pause = vi.fn(() => {
    paused = true
    audio.dispatchEvent(new Event('pause'))
  })
  return {
    advanceTo: value => {
      time = value
    },
    get time() {
      return time
    },
  }
}

async function renderWithTrack() {
  const view = render(
    <PlayerTab
      dropHandlerRef={{ current: null }}
      onConvertFiles={vi.fn()}
      openRequest={{ paths: ['/out/hearing.mp3'] }}
      onOpenRequestHandled={vi.fn()}
    />,
  )
  await screen.findByTestId('synced-transcript')
  const audio = view.container.querySelector('audio')
  return fakeAudio(audio)
}

const playPause = () => fireEvent.click(screen.getByRole('button', { name: /^(Play|Pause)$/ }))

describe('Player auto-backstep', () => {
  beforeEach(() => {
    window.localStorage.clear()
    vi.spyOn(window.HTMLMediaElement.prototype, 'load').mockImplementation(() => {})
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
  })

  it('shows the current backstep next to the transport, 2 s by default', async () => {
    await renderWithTrack()
    const select = screen.getByRole('combobox', { name: 'Backstep on resume' })
    expect(select).toHaveValue('2')
    expect(screen.getByRole('option', { name: 'Off' })).toBeInTheDocument()
    expect(screen.getByText('Backstep')).toBeInTheDocument()
  })

  it('rewinds 2 s when resuming after a pause', async () => {
    const clock = await renderWithTrack()
    playPause()
    clock.advanceTo(30)
    playPause()
    playPause()
    expect(clock.time).toBe(28)
  })

  it('does not rewind on the first play or after an explicit seek', async () => {
    const clock = await renderWithTrack()
    playPause()
    expect(clock.time).toBe(0)
    clock.advanceTo(30)
    playPause()
    fireEvent.keyDown(window, { key: 'ArrowRight' })
    expect(clock.time).toBe(35)
    playPause()
    expect(clock.time).toBe(35)
  })

  it('uses Space to resume with the same rule', async () => {
    const clock = await renderWithTrack()
    fireEvent.keyDown(window, { key: ' ' })
    clock.advanceTo(10)
    fireEvent.keyDown(window, { key: 'k' })
    fireEvent.keyDown(window, { key: ' ' })
    expect(clock.time).toBe(8)
  })

  it('persists the chosen backstep and honours Off', async () => {
    const clock = await renderWithTrack()
    fireEvent.change(screen.getByRole('combobox', { name: 'Backstep on resume' }), { target: { value: '0' } })
    expect(window.localStorage.getItem('player-backstep')).toBe('0')
    playPause()
    clock.advanceTo(30)
    playPause()
    playPause()
    expect(clock.time).toBe(30)
  })

  it('restores a saved backstep on launch', async () => {
    window.localStorage.setItem('player-backstep', '5')
    const clock = await renderWithTrack()
    expect(screen.getByRole('combobox', { name: 'Backstep on resume' })).toHaveValue('5')
    playPause()
    clock.advanceTo(3)
    playPause()
    playPause()
    expect(clock.time).toBe(0)
  })

  it('warns instead of failing when the setting cannot be saved', async () => {
    await renderWithTrack()
    vi.spyOn(window.Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    await act(async () => {
      fireEvent.change(screen.getByRole('combobox', { name: 'Backstep on resume' }), { target: { value: '3' } })
    })
    expect(screen.getByRole('combobox', { name: 'Backstep on resume' })).toHaveValue('3')
    expect(screen.getByText(/speed and backstep cannot be saved/)).toBeInTheDocument()
  })
})
