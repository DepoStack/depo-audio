import { act, cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import PlayerTab from '../components/Player/PlayerTab'
import { createPlaybackKeyDispatcher } from '../lib/playbackKeys'

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

function renderWithKeys({ holdToPlay = false, openRequest = { paths: ['/out/hearing.mp3'] } } = {}) {
  const dispatcher = createPlaybackKeyDispatcher({ onStatus: vi.fn() })
  dispatcher.setHoldToPlay(holdToPlay)
  const setTarget = vi.fn(target => dispatcher.setTarget(target))
  const view = render(
    <PlayerTab
      dropHandlerRef={{ current: null }}
      onConvertFiles={vi.fn()}
      openRequest={openRequest}
      onOpenRequestHandled={vi.fn()}
      playbackKeys={{ settings: { enabled: true }, setTarget }}
    />,
  )
  return { view, dispatcher, setTarget }
}

const settle = () => act(() => new Promise(resolve => setTimeout(resolve, 0)))
const tap = (dispatcher, action) =>
  act(() => {
    dispatcher.handle(action, 'Pressed')
    dispatcher.handle(action, 'Released')
  })

describe('Player wiring for playback keys while typing elsewhere', () => {
  beforeEach(() => window.localStorage.clear())
  afterEach(cleanup)

  it('says the keys do nothing until a recording is loaded, and clears its target on unmount', () => {
    const { view, setTarget } = renderWithKeys({ openRequest: null })
    expect(screen.getByText(/they do nothing until a recording is loaded/)).toBeInTheDocument()
    expect(setTarget).toHaveBeenLastCalledWith(expect.objectContaining({ hasTrack: false }))
    view.unmount()
    expect(setTarget).toHaveBeenLastCalledWith(null)
  })

  it('plays, pauses, seeks and changes speed through the Player transport', async () => {
    const { view, dispatcher } = renderWithKeys()
    await screen.findByTestId('synced-transcript')
    const audio = fakeAudio(view.container.querySelector('audio'))

    tap(dispatcher, 'playPause')
    expect(view.container.querySelector('audio').play).toHaveBeenCalledTimes(1)
    audio.advanceTo(30)
    tap(dispatcher, 'back')
    expect(audio.time).toBe(28)
    tap(dispatcher, 'forward')
    expect(audio.time).toBe(30)
    tap(dispatcher, 'playPause')
    expect(view.container.querySelector('audio').pause).toHaveBeenCalledTimes(1)
    tap(dispatcher, 'faster')
    expect(screen.getByRole('tab', { name: '1.25×' })).toHaveAttribute('aria-selected', 'true')
    tap(dispatcher, 'slower')
    expect(screen.getByRole('tab', { name: '1×' })).toHaveAttribute('aria-selected', 'true')
  })

  it('in hold mode plays while held, pauses on release, and resumes with the Player Backstep', async () => {
    window.localStorage.setItem('player-backstep', '3')
    const { view, dispatcher } = renderWithKeys({ holdToPlay: true })
    await screen.findByTestId('synced-transcript')
    const element = view.container.querySelector('audio')
    const audio = fakeAudio(element)

    act(() => dispatcher.handle('playPause', 'Pressed'))
    expect(element.play).toHaveBeenCalledTimes(1)
    audio.advanceTo(20)
    act(() => dispatcher.handle('playPause', 'Released'))
    expect(element.pause).toHaveBeenCalledTimes(1)
    await settle()

    act(() => dispatcher.handle('playPause', 'Pressed'))
    expect(audio.time).toBe(17)
    expect(element.play).toHaveBeenCalledTimes(2)

    // Back uses the same Backstep amount
    tap(dispatcher, 'back')
    expect(audio.time).toBe(14)
  })
})
