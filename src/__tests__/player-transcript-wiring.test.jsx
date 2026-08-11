import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { open } from '@tauri-apps/plugin-dialog'
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
  default: ({ trackPath }) => <div data-testid="synced-transcript" data-track-path={trackPath} />,
}))

describe('Player transcript wiring', () => {
  beforeEach(() => {
    window.localStorage.clear()
    vi.spyOn(window.HTMLMediaElement.prototype, 'load').mockImplementation(() => {})
    open.mockResolvedValue(['C:\\recordings\\hearing.wav'])
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    vi.restoreAllMocks()
  })

  it('mounts the synced transcript editor for the active track', async () => {
    render(<PlayerTab dropHandlerRef={{ current: null }} onConvertFiles={vi.fn()} />)

    fireEvent.click(screen.getByRole('button', { name: /add audio files to the playlist/i }))

    expect(await screen.findByTestId('synced-transcript')).toHaveAttribute(
      'data-track-path',
      'C:\\recordings\\hearing.wav',
    )
  })
})
