import { useState } from 'react'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { open } from '@tauri-apps/plugin-dialog'
import ConvertTab from '../components/Convert/ConvertTab'
import PlayerTab from '../components/Player/PlayerTab'
import { TestPreferencesProvider } from '../hooks/PreferencesContext'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
  invoke: vi.fn(() => Promise.resolve()),
}))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(vi.fn())) }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('../hooks/PreferencesContext', async () => {
  const { createContext, useContext } = await import('react')
  const preferencesContext = createContext(null)
  return {
    TestPreferencesProvider: preferencesContext.Provider,
    usePreferencesContext: () => useContext(preferencesContext),
  }
})
vi.mock('../components/common/Waveform', () => ({ default: () => <div /> }))
vi.mock('../components/Player/Transcript', () => ({ default: () => <div /> }))
vi.mock('../lib/speakerColors', () => ({
  SPEAKER_COUNT: 4,
  useSpeakerColors: () => ['#111111'],
  speakerColorAt: () => '#111111',
}))

const files = [{ path: '/recordings/hearing.wav', name: 'hearing.wav', fmt: null }]

function ChannelNames({ mode }) {
  const [labels, setLabels] = useState(['', 'Witness'])
  const preferences = {
    mode,
    formatOut: 'wav',
    labels,
    setLabels,
    chanVols: [1, 1],
    outDir: '',
    rate: '48000',
    mp3Bitrate: 192,
    normalize: false,
    trim: false,
    fade: false,
    hpf: false,
    autoLevel: false,
    declip: false,
  }
  return (
    <TestPreferencesProvider value={preferences}>
      <ConvertTab files={files} caseName="" jobs={{}} converting={false} doneCount={0} failCount={0} />
    </TestPreferencesProvider>
  )
}

async function openBookmarkedTrack() {
  window.localStorage.setItem(
    'player-bookmarks',
    JSON.stringify([
      { trackPath: files[0].path, time: 5, label: '' },
      { trackPath: files[0].path, time: 75, label: 'Witness enters' },
    ]),
  )
  render(<PlayerTab />)
  fireEvent.click(screen.getByRole('button', { name: /add audio files to the playlist/i }))
  await screen.findByRole('textbox', { name: 'Track label for hearing.wav' })
}

describe('Persistent names for channel and bookmark controls', () => {
  beforeEach(() => {
    window.localStorage.clear()
    vi.spyOn(window.HTMLMediaElement.prototype, 'load').mockImplementation(() => {})
    vi.spyOn(window.HTMLMediaElement.prototype, 'pause').mockImplementation(() => {})
    vi.spyOn(window.HTMLMediaElement.prototype, 'play').mockResolvedValue(undefined)
    open.mockResolvedValue([files[0].path])
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    vi.restoreAllMocks()
  })

  it.each(['stereo', 'split'])('names each channel independently of its editable value in %s mode', mode => {
    render(<ChannelNames mode={mode} />)
    const first = screen.getByRole('textbox', { name: 'Channel 1 name' })
    const second = screen.getByRole('textbox', { name: 'Channel 2 name' })
    expect(first).toHaveValue('')
    expect(second).toHaveValue('Witness')
    fireEvent.change(first, { target: { value: 'Reporter' } })
    fireEvent.change(second, { target: { value: '' } })
    expect(screen.getByRole('textbox', { name: 'Channel 1 name' })).toHaveValue('Reporter')
    expect(screen.getByRole('textbox', { name: 'Channel 2 name' })).toHaveValue('')
  })

  it('keeps bookmark notes named by their timestamp while notes are edited or cleared', async () => {
    await openBookmarkedTrack()
    const first = screen.getByRole('textbox', { name: 'Bookmark note at 0:05' })
    const second = screen.getByRole('textbox', { name: 'Bookmark note at 1:15' })
    expect(first).toHaveValue('')
    expect(second).toHaveValue('Witness enters')
    fireEvent.change(first, { target: { value: 'Opening' } })
    fireEvent.change(second, { target: { value: '' } })
    expect(screen.getByRole('textbox', { name: 'Bookmark note at 0:05' })).toHaveValue('Opening')
    expect(screen.getByRole('textbox', { name: 'Bookmark note at 1:15' })).toHaveValue('')
  })

  it('identifies the target of each remove action and removes only that bookmark', async () => {
    await openBookmarkedTrack()
    fireEvent.click(screen.getByRole('button', { name: 'Remove bookmark at 0:05' }))
    expect(screen.queryByRole('textbox', { name: 'Bookmark note at 0:05' })).not.toBeInTheDocument()
    expect(screen.getByRole('textbox', { name: 'Bookmark note at 1:15' })).toHaveValue('Witness enters')
    expect(screen.getByRole('button', { name: 'Remove bookmark at 1:15' })).toBeVisible()
    expect(JSON.parse(window.localStorage.getItem('player-bookmarks'))).toEqual([
      { trackPath: files[0].path, time: 75, label: 'Witness enters' },
    ])
  })
})
