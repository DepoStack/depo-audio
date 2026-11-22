import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { open } from '@tauri-apps/plugin-dialog'
import PlayerTab from '../components/Player/PlayerTab'
import { LAYOUT_KEY } from '../lib/playerLayout'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
  invoke: vi.fn(() => Promise.resolve(null)),
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

// The real Transcript puts its flags in the right pane through a portal; the
// stand-in does the same so the test can see where they land.
vi.mock('../components/Player/Transcript', async () => {
  const { createPortal } = await import('react-dom')
  return {
    default: ({ flagsContainer }) => (
      <div data-testid="synced-transcript">
        <div role="listbox" aria-label="Transcript lines" />
        {flagsContainer && createPortal(<p>Flags for review</p>, flagsContainer)}
      </div>
    ),
  }
})

function setWidth(width) {
  window.innerWidth = width
  act(() => {
    window.dispatchEvent(new Event('resize'))
  })
}

async function renderWithTrack() {
  const utils = render(<PlayerTab dropHandlerRef={{ current: null }} onConvertFiles={vi.fn()} />)
  fireEvent.click(screen.getByRole('button', { name: /add audio files to the playlist/i }))
  await screen.findByTestId('synced-transcript')
  return utils
}

const savedLayout = () => JSON.parse(window.localStorage.getItem(LAYOUT_KEY))

beforeEach(() => {
  window.innerWidth = 1280
  window.localStorage.clear()
  vi.spyOn(window.HTMLMediaElement.prototype, 'load').mockImplementation(() => {})
  open.mockResolvedValue(['/rec/hearing.wav'])
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  vi.restoreAllMocks()
  window.innerWidth = 1280
})

describe('Player three-pane layout', () => {
  it('puts the playlist left, the transcript in the centre and Now Playing with the flags right', async () => {
    await renderWithTrack()
    const left = screen.getByRole('complementary', { name: 'Playlist and bookmarks' })
    const centre = screen.getByRole('region', { name: 'Transcript' })
    const right = screen.getByRole('complementary', { name: 'Now playing, listening mix and flags' })
    expect(within(left).getByRole('heading', { name: 'Playlist' })).toBeVisible()
    expect(within(left).getByRole('heading', { name: 'Bookmarks' })).toBeVisible()
    expect(within(centre).getByRole('listbox', { name: 'Transcript lines' })).toBeVisible()
    expect(within(right).getByRole('heading', { name: 'Now Playing' })).toBeVisible()
    expect(within(right).getByRole('region', { name: 'Listening mix' })).toBeVisible()
    expect(within(right).getByText('Flags for review')).toBeVisible()
    expect(within(right).getByTestId('waveform')).toBeInTheDocument()
  })

  it('gives the transcript the largest share of the width', async () => {
    await renderWithTrack()
    const columns = screen.getByTestId('player-panes').style.gridTemplateColumns.split(' ')
    expect(columns).toContain('minmax(0,1fr)')
    const fixed = columns.filter(c => c.endsWith('px')).map(c => parseFloat(c))
    const [left, , , right] = fixed
    const centre = 1280 - fixed.reduce((sum, w) => sum + w, 0)
    expect(centre).toBeGreaterThan(left)
    expect(centre).toBeGreaterThan(right)
    expect(centre).toBeGreaterThanOrEqual(1280 * 0.5)
  })

  it('resizes a pane from the keyboard without seeking the audio, and remembers it', async () => {
    const { unmount } = await renderWithTrack()
    const audio = document.querySelector('audio')
    const seek = vi.fn()
    Object.defineProperty(audio, 'currentTime', { configurable: true, get: () => 30, set: seek })
    const splitter = screen.getByRole('separator', { name: 'Resize the playlist and bookmarks pane' })
    expect(splitter).toHaveAttribute('aria-valuenow', '220')
    expect(splitter).toHaveAttribute('aria-orientation', 'vertical')
    fireEvent.keyDown(splitter, { key: 'ArrowRight' })
    expect(splitter).toHaveAttribute('aria-valuenow', '236')
    expect(seek).not.toHaveBeenCalled()
    fireEvent.keyDown(splitter, { key: 'End' })
    expect(splitter).toHaveAttribute('aria-valuenow', '320')
    const right = screen.getByRole('separator', { name: 'Resize the now playing and flags pane' })
    fireEvent.keyDown(right, { key: 'ArrowLeft' })
    expect(right).toHaveAttribute('aria-valuenow', '296')
    expect(savedLayout()).toMatchObject({ left: 320, right: 296 })

    unmount()
    await renderWithTrack()
    expect(screen.getByRole('separator', { name: 'Resize the playlist and bookmarks pane' })).toHaveAttribute(
      'aria-valuenow',
      '320',
    )
  })

  it('resizes a pane by dragging its splitter', async () => {
    await renderWithTrack()
    const splitter = screen.getByRole('separator', { name: 'Resize the playlist and bookmarks pane' })
    fireEvent.pointerDown(splitter, { button: 0, clientX: 220, pointerId: 1 })
    fireEvent.pointerMove(splitter, { clientX: 290, pointerId: 1 })
    expect(splitter).toHaveAttribute('aria-valuenow', '290')
    expect(window.localStorage.getItem(LAYOUT_KEY)).toBeNull()
    fireEvent.pointerUp(splitter, { clientX: 290, pointerId: 1 })
    expect(savedLayout()).toMatchObject({ left: 290 })
  })

  it('collapses a side pane and keeps it collapsed next time', async () => {
    const { unmount } = await renderWithTrack()
    fireEvent.click(screen.getByRole('button', { name: 'Hide playlist and bookmarks' }))
    const show = screen.getByRole('button', { name: 'Show playlist and bookmarks' })
    expect(show).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('heading', { name: 'Playlist' })).not.toBeInTheDocument()
    expect(screen.queryByRole('separator', { name: /playlist/ })).not.toBeInTheDocument()
    expect(savedLayout()).toMatchObject({ leftOpen: false, rightOpen: true })

    unmount()
    await renderWithTrack()
    fireEvent.click(screen.getByRole('button', { name: 'Show playlist and bookmarks' }))
    expect(screen.getByRole('heading', { name: 'Playlist' })).toBeVisible()
    expect(savedLayout()).toMatchObject({ leftOpen: true })
  })

  it('says so when the pane sizes cannot be saved', async () => {
    await renderWithTrack()
    vi.spyOn(window.Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    fireEvent.click(screen.getByRole('button', { name: 'Hide now playing, listening mix and flags' }))
    expect(screen.getByRole('button', { name: 'Show now playing, listening mix and flags' })).toBeVisible()
    expect(await screen.findByText(/pane sizes cannot be saved/)).toBeInTheDocument()
  })

  it('keeps the playback keys working with the panes', async () => {
    await renderWithTrack()
    const audio = document.querySelector('audio')
    let time = 20
    Object.defineProperty(audio, 'currentTime', {
      configurable: true,
      get: () => time,
      set: v => {
        time = v
      },
    })
    Object.defineProperty(audio, 'duration', { configurable: true, get: () => 120 })
    fireEvent.keyDown(document.body, { key: 'l' })
    expect(time).toBe(30)
    fireEvent.keyDown(document.body, { key: 'ArrowLeft' })
    expect(time).toBe(25)
    fireEvent.keyDown(document.body, { key: 'b' })
    expect(
      within(screen.getByRole('complementary', { name: 'Playlist and bookmarks' })).getByText('0:25'),
    ).toBeVisible()
  })
})

describe('Player in a narrow window', () => {
  it('turns the side panes into drawers and keeps the transcript in the middle', async () => {
    window.innerWidth = 900
    await renderWithTrack()
    expect(screen.getByTestId('player-workspace')).toHaveAttribute('data-layout', 'narrow')
    expect(screen.getByRole('listbox', { name: 'Transcript lines' })).toBeVisible()
    expect(screen.queryByRole('separator')).not.toBeInTheDocument()
    const playlistButton = screen.getByRole('button', { name: 'Playlist and bookmarks' })
    const nowButton = screen.getByRole('button', { name: 'Now playing and flags' })
    expect(playlistButton).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('heading', { name: 'Playlist' })).not.toBeInTheDocument()

    fireEvent.click(playlistButton)
    expect(playlistButton).toHaveAttribute('aria-expanded', 'true')
    const drawer = screen.getByRole('region', { name: 'Playlist and bookmarks' })
    expect(within(drawer).getByRole('heading', { name: 'Playlist' })).toBeVisible()
    expect(drawer).toContainElement(document.activeElement)

    // Opening the other drawer closes this one
    fireEvent.click(nowButton)
    expect(playlistButton).toHaveAttribute('aria-expanded', 'false')
    const nowDrawer = screen.getByRole('region', { name: 'Now playing, listening mix and flags' })
    expect(within(nowDrawer).getByText('Flags for review')).toBeVisible()

    fireEvent.keyDown(document.activeElement, { key: 'Escape' })
    expect(nowButton).toHaveAttribute('aria-expanded', 'false')
    expect(nowButton).toHaveFocus()
    expect(screen.queryByRole('heading', { name: 'Now Playing' })).not.toBeInTheDocument()
  })

  it('switches between panes and drawers when the window is resized', async () => {
    await renderWithTrack()
    expect(screen.getByTestId('player-workspace')).toHaveAttribute('data-layout', 'panes')
    setWidth(1000)
    expect(screen.getByTestId('player-workspace')).toHaveAttribute('data-layout', 'narrow')
    fireEvent.click(screen.getByRole('button', { name: 'Playlist and bookmarks' }))
    fireEvent.click(screen.getByRole('button', { name: 'Close playlist and bookmarks' }))
    expect(screen.getByRole('button', { name: 'Playlist and bookmarks' })).toHaveFocus()
    setWidth(1440)
    expect(screen.getByTestId('player-workspace')).toHaveAttribute('data-layout', 'panes')
    expect(screen.getByRole('heading', { name: 'Playlist' })).toBeVisible()
  })
})
