import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import PlayerTab from '../components/Player/PlayerTab'
import { installFakeAudio } from './helpers/fakeAudioContext'

const channelsByPath = { '/out/four.wav': 4, '/out/six.wav': 6, '/out/stereo.wav': 2, '/out/mono.wav': 1 }

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
  invoke: vi.fn((command, { path }) => {
    if (command !== 'probe_audio_streams_cmd') return Promise.resolve(null)
    if (path === '/out/unprobed.wav') return Promise.reject(new Error('ffprobe failed'))
    return Promise.resolve({ convertedIndex: 0, streams: [{ channels: channelsByPath[path] }] })
  }),
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

function fakeElement(audio) {
  let paused = true
  let time = 0
  Object.defineProperty(audio, 'paused', { configurable: true, get: () => paused })
  Object.defineProperty(audio, 'duration', { configurable: true, get: () => 120 })
  Object.defineProperty(audio, 'currentTime', {
    configurable: true,
    get: () => time,
    set: v => {
      time = v
    },
  })
  audio.load = vi.fn()
  audio.play = vi.fn(() => {
    paused = false
    audio.dispatchEvent(new Event('play'))
    return Promise.resolve()
  })
  audio.pause = vi.fn(() => {
    if (paused) return
    paused = true
    audio.dispatchEvent(new Event('pause'))
  })
  return audio
}

async function renderPlayer(paths = ['/out/four.wav']) {
  const view = render(
    <PlayerTab
      dropHandlerRef={{ current: null }}
      onConvertFiles={vi.fn()}
      openRequest={{ paths }}
      onOpenRequestHandled={vi.fn()}
    />,
  )
  await screen.findByTestId('synced-transcript')
  await settle()
  const audio = () => fakeElement(view.container.querySelector('audio'))
  audio()
  return { view, audio: () => view.container.querySelector('audio') }
}

describe('Player listening mix', () => {
  let fake
  beforeEach(() => {
    window.localStorage.clear()
    fake = installFakeAudio()
  })
  afterEach(() => {
    cleanup()
    fake.restore()
  })

  it('has a master volume that sets the element volume and is saved', async () => {
    const { audio } = await renderPlayer()
    const slider = screen.getByRole('slider', { name: 'Volume' })
    expect(slider).toHaveAttribute('aria-valuetext', '100%')
    expect(slider).toHaveAccessibleDescription(/Changes what you hear only/)
    fireEvent.change(slider, { target: { value: '0.4' } })
    expect(audio().volume).toBeCloseTo(0.4)
    expect(window.localStorage.getItem('player-volume')).toBe('0.4')
    expect(fake.contexts).toHaveLength(0) // plain playback does not use Web Audio
  })

  it('restores the saved volume', async () => {
    window.localStorage.setItem('player-volume', '0.25')
    const { audio } = await renderPlayer()
    expect(screen.getByRole('slider', { name: 'Volume' })).toHaveValue('0.25')
    expect(audio().volume).toBeCloseTo(0.25)
  })

  it('offers Separate channels only for files with 2 or more channels', async () => {
    await renderPlayer(['/out/mono.wav'])
    expect(screen.queryByRole('button', { name: 'Separate channels' })).toBeNull()
  })

  it('turns on a per-channel mix through Web Audio and back off', async () => {
    const { audio } = await renderPlayer()
    const plain = audio()
    expect(plain).not.toHaveAttribute('crossorigin')
    const toggle = screen.getByRole('button', { name: 'Separate channels' })
    expect(toggle).toHaveAttribute('aria-pressed', 'false')
    fireEvent.click(toggle)
    await settle()

    const mixed = audio()
    expect(mixed).not.toBe(plain)
    expect(mixed).toHaveAttribute('crossorigin', 'anonymous')
    expect(toggle).toHaveAttribute('aria-pressed', 'true')
    expect(fake.contexts).toHaveLength(1)
    const context = fake.contexts[0]
    expect(context.nodes.find(n => n.kind === 'source').element).toBe(mixed)
    expect(context.resume).toHaveBeenCalled()
    expect(screen.getAllByTestId('channel-strip')).toHaveLength(4)
    expect(mixed.volume).toBe(1) // master gain does the volume now

    fireEvent.click(screen.getByRole('button', { name: 'Solo Ch 2' }))
    fireEvent.click(screen.getByRole('button', { name: 'Solo Ch 3' }))
    fireEvent.click(screen.getByRole('button', { name: 'Mute Ch 3' }))
    fireEvent.change(screen.getByRole('slider', { name: 'Ch 2 volume' }), { target: { value: '1.5' } })
    expect(fake.channelGains(context).map(g => g.gain.value)).toEqual([0, 1.5, 0, 0])
    expect(screen.getByRole('button', { name: 'Mute Ch 3' })).toHaveAttribute('aria-pressed', 'true')
    const ch1 = screen.getAllByTestId('channel-strip')[0]
    expect(within(ch1).getByText('Silent (solo)')).toBeInTheDocument()

    fireEvent.change(screen.getByRole('slider', { name: 'Volume' }), { target: { value: '0.5' } })
    expect(fake.master(context).gain.value).toBe(0.5)

    const reset = screen.getByRole('button', { name: 'Reset channels' })
    reset.focus()
    fireEvent.click(reset)
    // 4 channels share the level in both ears.
    expect(fake.channelGains(context).map(g => g.gain.value)).toEqual([0.25, 0.25, 0.25, 0.25])
    // The button stays (so focus does too) and the reset is announced.
    expect(document.activeElement).toBe(reset)
    expect(reset).toHaveAttribute('aria-disabled', 'true')
    expect(screen.getByRole('status')).toHaveTextContent('Channels reset. Every channel is heard at 100%.')
    fireEvent.click(reset)
    expect(screen.getByRole('status')).toHaveTextContent('Channels reset.')

    fireEvent.click(toggle)
    await settle()
    expect(context.close).toHaveBeenCalled()
    expect(audio()).not.toHaveAttribute('crossorigin')
    expect(audio().volume).toBeCloseTo(0.5)
    expect(screen.queryAllByTestId('channel-strip')).toHaveLength(0)
  })

  it('keeps the position and play state across the swap', async () => {
    const { audio } = await renderPlayer()
    const plain = audio()
    plain.currentTime = 42
    plain.play()
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    const mixed = fakeElement(audio())
    fireEvent(mixed, new Event('loadedmetadata'))
    expect(mixed.currentTime).toBe(42)
    expect(mixed.play).toHaveBeenCalled()
  })

  it('says when everything is silenced', async () => {
    await renderPlayer(['/out/stereo.wav'])
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    fireEvent.click(screen.getByRole('button', { name: 'Mute Ch 1' }))
    fireEvent.click(screen.getByRole('button', { name: 'Mute Ch 2' }))
    expect(screen.getByRole('status')).toHaveTextContent(/Every channel is silent/)
  })

  it('shows only the channels the web view passes, with a note', async () => {
    let now = 0
    vi.spyOn(performance, 'now').mockImplementation(() => now)
    const { audio } = await renderPlayer()
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    const mixed = fakeElement(audio())
    expect(screen.getByText(/Play the file to check/)).toBeInTheDocument()
    mixed.play()
    now = 1000
    await act(async () => {
      fake.workletNodes.at(-1).port.onmessage({ data: { channels: 2 } })
    })
    expect(screen.getAllByTestId('channel-strip')).toHaveLength(2)
    expect(screen.getByRole('button', { name: 'Mute Left mix' })).toBeInTheDocument()
    expect(
      screen.getByText(
        'This file has 4 channels; the web view mixes them down, so the Player can separate 2 here. Use Split Channels in Convert to hear each channel.',
      ),
    ).toBeInTheDocument()
    expect(fake.channelGains(fake.contexts[0])).toHaveLength(2)
    vi.restoreAllMocks()
  })

  it('resets the strips for a new track and closes the mix on unmount', async () => {
    const { view } = await renderPlayer(['/out/four.wav', '/out/stereo.wav'])
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    fireEvent.click(screen.getByRole('button', { name: 'Solo Ch 1' }))
    const context = fake.contexts[0]
    expect(fake.channelGains(context).map(g => g.gain.value)).toEqual([1, 0, 0, 0])
    fireEvent.click(screen.getByRole('button', { name: 'Next track' }))
    await settle()
    expect(screen.getAllByTestId('channel-strip')).toHaveLength(2)
    expect(screen.getByRole('button', { name: 'Solo Ch 1' })).toHaveAttribute('aria-pressed', 'false')
    expect(fake.channelGains(context).map(g => g.gain.value)).toEqual([1, 1])
    view.unmount()
    expect(context.close).toHaveBeenCalled()
  })

  it('goes back to plain playback when the element cannot be opened for Web Audio', async () => {
    const { audio } = await renderPlayer()
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    fireEvent(audio(), new Event('error'))
    await settle()
    expect(screen.getByText(/Separate channels could not start/)).toBeInTheDocument()
    expect(audio()).not.toHaveAttribute('crossorigin')
    // Pressing Play does not hide why the strips went away.
    fakeElement(audio())
    fireEvent.click(screen.getByRole('button', { name: 'Play' }))
    expect(screen.getByText(/Separate channels could not start/)).toBeInTheDocument()
    expect(fake.contexts[0].close).toHaveBeenCalled()
  })

  it('announces how many channels are heard and ties the listening-only note to the strips', async () => {
    await renderPlayer()
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    const group = screen.getByRole('group', { name: 'Channel strips' })
    expect(group).toHaveAccessibleDescription(/Changes what you hear only/)
    const status = screen.getByRole('status')
    expect(status).toHaveTextContent('')
    fireEvent.click(screen.getByRole('button', { name: 'Solo Ch 2' }))
    expect(status).toHaveTextContent('Hearing 1 of 4 channels.')
    fireEvent.click(screen.getByRole('button', { name: 'Mute Ch 4' }))
    expect(status).toHaveTextContent('Hearing 1 of 4 channels.')
    fireEvent.click(screen.getByRole('button', { name: 'Solo Ch 2' }))
    expect(status).toHaveTextContent('Hearing 3 of 4 channels.')
    // Messages come before the strips, so they stay in view with many channels.
    expect(status.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // Reset channels is there but has nothing to do until a strip changes.
    fireEvent.click(screen.getByRole('button', { name: 'Mute Ch 4' }))
    expect(screen.getByRole('button', { name: 'Reset channels' })).toHaveAttribute('aria-disabled', 'true')
  })

  it('clears a Solo set before the web view turned out to mix the file down', async () => {
    let now = 0
    vi.spyOn(performance, 'now').mockImplementation(() => now)
    const { audio } = await renderPlayer(['/out/six.wav'])
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    expect(screen.getAllByTestId('channel-strip')).toHaveLength(6)
    fireEvent.click(screen.getByRole('button', { name: 'Solo Ch 5' }))
    const context = fake.contexts[0]
    expect(fake.channelGains(context).map(g => g.gain.value)).toEqual([0, 0, 0, 0, 1, 0])
    const mixed = fakeElement(audio())
    mixed.play()
    now = 1000
    await act(async () => {
      fake.workletNodes.at(-1).port.onmessage({ data: { channels: 2 } })
    })
    expect(screen.getAllByTestId('channel-strip')).toHaveLength(2)
    // Both halves of the down-mix are heard: the hidden Solo is gone.
    expect(fake.channelGains(context).map(g => g.gain.value)).toEqual([1, 1])
    expect(screen.getByRole('button', { name: 'Solo Left mix' })).toHaveAttribute('aria-pressed', 'false')
    expect(screen.getByRole('status')).toHaveTextContent(
      'The web view mixes this file down, so the Mute, Solo and volume settings you made for its channels were cleared.',
    )
    // Turning the mix off and on does not bring the hidden Solo back.
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    const again = fake.contexts.at(-1)
    expect(fake.channelGains(again).every(g => g.gain.value > 0)).toBe(true)
    // A change on the visible strips replaces the message.
    fireEvent.click(screen.getAllByRole('button', { name: /^Mute / })[0])
    expect(screen.getByRole('status')).toHaveTextContent('Hearing 1 of 2 channels.')
    vi.restoreAllMocks()
  })

  it('keeps Separate channels reachable when the next track has one channel', async () => {
    await renderPlayer(['/out/stereo.wav', '/out/mono.wav'])
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    fireEvent.click(screen.getByRole('button', { name: 'Next track' }))
    await settle()
    const toggle = screen.getByRole('button', { name: 'Separate channels' })
    expect(toggle).toHaveAttribute('aria-pressed', 'true')
    expect(screen.queryAllByTestId('channel-strip')).toHaveLength(0)
    expect(screen.queryByRole('button', { name: 'Mute Ch 1' })).toBeNull()
    expect(screen.getByText(/This track has 1 channel, so there is nothing to separate/)).toBeInTheDocument()
    expect(fake.channelGains(fake.contexts[0]).map(g => g.gain.value)).toEqual([1])
    fireEvent.click(toggle)
    await settle()
    expect(document.querySelector('audio')).not.toHaveAttribute('crossorigin')
    expect(screen.queryByRole('button', { name: 'Separate channels' })).toBeNull()
  })

  it('keeps Separate channels reachable when the next track cannot be probed', async () => {
    await renderPlayer(['/out/stereo.wav', '/out/unprobed.wav'])
    fireEvent.click(screen.getByRole('button', { name: 'Separate channels' }))
    await settle()
    fireEvent.click(screen.getByRole('button', { name: 'Next track' }))
    await settle()
    expect(screen.getByRole('button', { name: 'Separate channels' })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByText(/could not count this track's channels/)).toBeInTheDocument()
    expect(screen.queryAllByTestId('channel-strip')).toHaveLength(0)
  })
})
