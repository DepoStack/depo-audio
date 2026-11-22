import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import PlayerTab from '../components/Player/PlayerTab'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
  invoke: vi.fn(() => Promise.resolve(null)),
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
    // A real media element queues 'seeking' as a task after currentTime is
    // set, so code that sets the time and then plays sees it afterwards.
    set: value => {
      time = value
      setTimeout(() => audio.dispatchEvent(new Event('seeking')), 0)
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
// Let queued media events (seeking) run.
const settle = () => act(() => new Promise(resolve => setTimeout(resolve, 0)))
const chooseBackstep = value =>
  fireEvent.change(screen.getByRole('combobox', { name: 'Backstep on resume' }), { target: { value } })

describe('Player auto-backstep', () => {
  beforeEach(() => {
    window.localStorage.clear()
    vi.spyOn(window.HTMLMediaElement.prototype, 'load').mockImplementation(() => {})
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
  })

  it('is Off by default, next to the transport, with its rule as the description', async () => {
    const clock = await renderWithTrack()
    const select = screen.getByRole('combobox', { name: 'Backstep on resume' })
    expect(select).toHaveValue('0')
    expect(select).toHaveAccessibleDescription(
      'When you resume after a pause, the Player first rewinds this far. Seeks and track changes do not rewind.',
    )
    expect(screen.getByRole('option', { name: 'Off' })).toBeInTheDocument()
    expect(screen.getByText('Backstep')).toBeInTheDocument()
    // Off: an existing install's playback does not change after the update.
    playPause()
    clock.advanceTo(30)
    playPause()
    await settle()
    playPause()
    expect(clock.time).toBe(30)
  })

  it('rewinds 2 s when resuming after a pause', async () => {
    const clock = await renderWithTrack()
    chooseBackstep('2')
    playPause()
    clock.advanceTo(30)
    playPause()
    await settle()
    playPause()
    expect(clock.time).toBe(28)
  })

  it('keeps rewinding on later pauses after its own seek event arrives', async () => {
    const clock = await renderWithTrack()
    chooseBackstep('2')
    playPause()
    clock.advanceTo(30)
    playPause()
    playPause()
    expect(clock.time).toBe(28)
    // The 'seeking' from that rewind arrives after play(); it must not eat
    // the next pause.
    await settle()
    clock.advanceTo(40)
    playPause()
    await settle()
    playPause()
    expect(clock.time).toBe(38)
  })

  it('does not rewind on the first play or after an explicit seek', async () => {
    const clock = await renderWithTrack()
    chooseBackstep('2')
    playPause()
    expect(clock.time).toBe(0)
    clock.advanceTo(30)
    playPause()
    fireEvent.keyDown(window, { key: 'ArrowRight' })
    expect(clock.time).toBe(35)
    await settle()
    playPause()
    expect(clock.time).toBe(35)
  })

  it('uses Space to resume with the same rule', async () => {
    const clock = await renderWithTrack()
    chooseBackstep('2')
    fireEvent.keyDown(window, { key: ' ' })
    clock.advanceTo(10)
    fireEvent.keyDown(window, { key: 'k' })
    fireEvent.keyDown(window, { key: ' ' })
    expect(clock.time).toBe(8)
  })

  it('persists the chosen backstep and honours Off', async () => {
    const clock = await renderWithTrack()
    chooseBackstep('3')
    expect(window.localStorage.getItem('player-backstep')).toBe('3')
    chooseBackstep('0')
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
    expect(screen.getByText(/speed, volume and backstep cannot be saved/)).toBeInTheDocument()
  })
})
