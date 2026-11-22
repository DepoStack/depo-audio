import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import PlayerTab from '../components/Player/PlayerTab'
import { createPlaybackKeyDispatcher } from '../lib/playbackKeys'
import { installFakeAudio } from './helpers/fakeAudioContext'

// Playback keys while typing elsewhere, with Separate channels on: the keys
// drive the <audio> element that feeds Web Audio, the mixer's context is
// resumed on play, and Backstep and hold-to-play behave as without the mix.

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
  invoke: vi.fn(command =>
    command === 'probe_audio_streams_cmd'
      ? Promise.resolve({ convertedIndex: 0, streams: [{ channels: 4 }] })
      : Promise.resolve(null),
  ),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

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

const settle = () => act(() => new Promise(resolve => setTimeout(resolve, 0)))

// Like a real media element: play/pause fire events only on a change of
// state, and setting currentTime queues 'seeking' as a task.
function fakeElement(audio) {
  if (audio.__fake) return audio
  let paused = true
  let time = 0
  Object.defineProperty(audio, 'paused', { configurable: true, get: () => paused })
  Object.defineProperty(audio, 'ended', { configurable: true, get: () => false })
  Object.defineProperty(audio, 'duration', { configurable: true, get: () => 120 })
  Object.defineProperty(audio, 'currentTime', {
    configurable: true,
    get: () => time,
    set: v => {
      time = v
      setTimeout(() => audio.dispatchEvent(new Event('seeking')), 0)
    },
  })
  audio.load = vi.fn()
  audio.play = vi.fn(() => {
    if (paused) {
      paused = false
      audio.dispatchEvent(new Event('play'))
    }
    return Promise.resolve()
  })
  audio.pause = vi.fn(() => {
    if (paused) return
    paused = true
    audio.dispatchEvent(new Event('pause'))
  })
  audio.__fake = true
  audio.advanceTo = v => {
    time = v
  }
  return audio
}

async function renderMixed({ holdToPlay = false } = {}) {
  const dispatcher = createPlaybackKeyDispatcher({ onStatus: vi.fn() })
  dispatcher.setHoldToPlay(holdToPlay)
  const view = render(
    <PlayerTab
      dropHandlerRef={{ current: null }}
      onConvertFiles={vi.fn()}
      openRequest={{ paths: ['/out/four.wav'] }}
      onOpenRequestHandled={vi.fn()}
      playbackKeys={{ settings: { enabled: true }, setTarget: target => dispatcher.setTarget(target) }}
    />,
  )
  await screen.findByTestId('synced-transcript')
  await settle()
  fakeElement(view.container.querySelector('audio'))
  const audio = () => fakeElement(view.container.querySelector('audio'))
  return { view, dispatcher, audio }
}

const tap = (dispatcher, action) =>
  act(() => {
    dispatcher.handle(action, 'Pressed')
    dispatcher.handle(action, 'Released')
  })

// Separate channels swaps in a new element; it restores on loadedmetadata.
async function turnMixOn(audio) {
  const before = audio()
  fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
  await settle()
  const mixed = audio()
  expect(mixed).not.toBe(before)
  return mixed
}

describe('Playback keys with Separate channels on', () => {
  let fake
  beforeEach(() => {
    window.localStorage.clear()
    fake = installFakeAudio()
  })
  afterEach(() => {
    cleanup()
    fake.restore()
  })

  it('plays, pauses, seeks and changes speed on the element that feeds Web Audio', async () => {
    window.localStorage.setItem('player-backstep', '3')
    const { dispatcher, audio } = await renderMixed()
    const mixed = await turnMixOn(audio)
    fireEvent(mixed, new Event('loadedmetadata'))
    const context = fake.contexts[0]
    expect(context.nodes.find(n => n.kind === 'source').element).toBe(mixed)
    // A suspended context (for example after the system audio device
    // changed) is resumed by the key, as by the Play button.
    context.state = 'suspended'
    context.resume.mockClear()

    tap(dispatcher, 'playPause')
    expect(mixed.play).toHaveBeenCalledTimes(1)
    expect(context.resume).toHaveBeenCalled()
    mixed.advanceTo(30)
    tap(dispatcher, 'back') // Backstep amount
    expect(mixed.currentTime).toBe(27)
    tap(dispatcher, 'forward')
    expect(mixed.currentTime).toBe(29)
    tap(dispatcher, 'faster')
    expect(mixed.playbackRate).toBe(1.25)
    expect(screen.getByRole('tab', { name: '1.25×' })).toHaveAttribute('aria-selected', 'true')
    tap(dispatcher, 'slower')
    expect(mixed.playbackRate).toBe(1)
    await settle()

    tap(dispatcher, 'playPause')
    expect(mixed.pause).toHaveBeenCalledTimes(1)
    await settle()
    tap(dispatcher, 'playPause')
    expect(mixed.currentTime).toBe(26) // resumed 3 s back
    expect(mixed.play).toHaveBeenCalledTimes(2)
    // The channel strips are untouched by the keys
    expect(screen.getAllByTestId('channel-strip')).toHaveLength(4)
  })

  it('in hold mode plays while held and resumes with Backstep through the mix', async () => {
    window.localStorage.setItem('player-backstep', '2')
    const { dispatcher, audio } = await renderMixed({ holdToPlay: true })
    const mixed = await turnMixOn(audio)
    fireEvent(mixed, new Event('loadedmetadata'))

    act(() => dispatcher.handle('playPause', 'Pressed'))
    expect(mixed.play).toHaveBeenCalledTimes(1)
    mixed.advanceTo(40)
    act(() => dispatcher.handle('playPause', 'Released'))
    expect(mixed.pause).toHaveBeenCalledTimes(1)
    await settle()
    act(() => dispatcher.handle('playPause', 'Pressed'))
    expect(mixed.currentTime).toBe(38)
    expect(mixed.play).toHaveBeenCalledTimes(2)
  })

  it('keeps the Backstep resume point when the mix is turned on while paused', async () => {
    window.localStorage.setItem('player-backstep', '3')
    const { dispatcher, audio } = await renderMixed()
    const plain = audio()
    tap(dispatcher, 'playPause')
    plain.advanceTo(50)
    tap(dispatcher, 'playPause')
    expect(plain.pause).toHaveBeenCalledTimes(1)
    await settle()

    const mixed = await turnMixOn(audio)
    fireEvent(mixed, new Event('loadedmetadata'))
    expect(mixed.currentTime).toBe(50)
    await settle() // the restore's own 'seeking' is not a person's seek

    tap(dispatcher, 'playPause')
    expect(mixed.currentTime).toBe(47)
    expect(mixed.play).toHaveBeenCalledTimes(1)
  })

  it('a key pressed before the swapped-in element has loaded changes what the restore does', async () => {
    window.localStorage.setItem('player-backstep', '2')
    const { dispatcher, audio } = await renderMixed({ holdToPlay: true })
    const plain = audio()
    act(() => dispatcher.handle('playPause', 'Pressed'))
    plain.advanceTo(12)

    const mixed = await turnMixOn(audio) // still held: the swap will resume playing
    act(() => dispatcher.handle('playPause', 'Released')) // released before metadata
    expect(mixed.play).not.toHaveBeenCalled()
    fireEvent(mixed, new Event('loadedmetadata'))
    expect(mixed.currentTime).toBe(12)
    expect(mixed.play).not.toHaveBeenCalled() // stays paused, as the pedal says
    expect(mixed.paused).toBe(true)
    await settle()

    // The release counted as a pause at 12 s, so the next press rewinds
    act(() => dispatcher.handle('playPause', 'Pressed'))
    expect(mixed.currentTime).toBe(10)
    expect(mixed.play).toHaveBeenCalledTimes(1)
  })
  it('applies a Back press made before the swapped-in element has loaded', async () => {
    window.localStorage.setItem('player-backstep', '3')
    const { dispatcher, audio } = await renderMixed()
    const plain = audio()
    tap(dispatcher, 'playPause')
    plain.advanceTo(40)

    const mixed = await turnMixOn(audio)
    tap(dispatcher, 'back') // 3 s back, before metadata
    tap(dispatcher, 'forward') // then 2 s forward
    expect(mixed.play).not.toHaveBeenCalled()
    fireEvent(mixed, new Event('loadedmetadata'))
    expect(mixed.currentTime).toBe(39)
    expect(mixed.play).toHaveBeenCalledTimes(1)
  })

  it('goes back to normal playback when a key plays into a context that will not run', async () => {
    const { dispatcher, audio } = await renderMixed()
    const mixed = await turnMixOn(audio)
    fireEvent(mixed, new Event('loadedmetadata'))
    const context = fake.contexts[0]
    // A context the web view keeps suspended (no autoplay without a gesture)
    context.state = 'suspended'
    context.resume.mockImplementation(() => Promise.resolve())
    tap(dispatcher, 'playPause')
    await settle()
    await settle()
    expect(
      screen.getByText(/Separate channels could not start \(the web view kept Web Audio paused/),
    ).toBeInTheDocument()
    expect(audio()).not.toHaveAttribute('crossorigin')
    expect(screen.getByRole('button', { name: 'Separate channels' })).toHaveAttribute('aria-pressed', 'false')
  })

  it('resumes a context WebKit reports as interrupted', async () => {
    const { dispatcher, audio } = await renderMixed()
    const mixed = await turnMixOn(audio)
    fireEvent(mixed, new Event('loadedmetadata'))
    const context = fake.contexts[0]
    context.state = 'interrupted'
    context.resume.mockClear()
    tap(dispatcher, 'playPause')
    await settle()
    expect(context.resume).toHaveBeenCalled()
    expect(screen.queryByText(/Separate channels could not start/)).toBeNull()
    expect(screen.getByRole('button', { name: 'Separate channels' })).toHaveAttribute('aria-pressed', 'true')
  })
})
