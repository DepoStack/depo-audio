import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import FileRow from '../components/Convert/FileRow'
import { isPlayerPlayable } from '../lib/player'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  convertFileSrc: path => `asset://${path}`,
}))

const file = { path: '/case/hearing.trm', name: 'hearing.trm', fmt: null }

function doneJob(outputs) {
  return { status: 'done', outputs }
}

describe('Completed conversion rows', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue()
  })
  afterEach(cleanup)

  it('offers Reveal and Open in Player for a single output', async () => {
    const onOpenInPlayer = vi.fn()
    render(
      <FileRow
        file={file}
        job={doneJob([{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1024 }])}
        onRemove={vi.fn()}
        converting={false}
        onOpenInPlayer={onOpenInPlayer}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Reveal in folder: hearing.mp3' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: '/out/hearing.mp3' }))

    fireEvent.click(screen.getByRole('button', { name: 'Open in Player: hearing.mp3' }))
    expect(onOpenInPlayer).toHaveBeenCalledWith(['/out/hearing.mp3'])
  })

  it('offers the same actions for split outputs', async () => {
    const onOpenInPlayer = vi.fn()
    const outputs = [
      { path: '/out/hearing_ch1.wav', name: 'hearing_ch1.wav', size: 1 },
      { path: '/out/hearing_ch2.wav', name: 'hearing_ch2.wav', size: 1 },
    ]
    render(
      <FileRow
        file={file}
        job={doneJob(outputs)}
        onRemove={vi.fn()}
        converting={false}
        onOpenInPlayer={onOpenInPlayer}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Reveal in folder: the 2 outputs of hearing.trm' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: '/out/hearing_ch1.wav' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open in Player: the 2 outputs of hearing.trm' }))
    expect(onOpenInPlayer).toHaveBeenCalledWith(outputs.map(out => out.path))
  })

  it('shows a reveal failure instead of hiding it', async () => {
    invoke.mockRejectedValue(new Error('opener unavailable'))
    render(
      <FileRow
        file={file}
        job={doneJob([{ path: '/out/hearing.flac', name: 'hearing.flac', size: 1 }])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Reveal in folder: hearing.flac' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('opener unavailable')
    // Without a Player handler there is no Open in Player button
    expect(screen.queryByRole('button', { name: /Open in Player/ })).not.toBeInTheDocument()
  })

  it('gives each action an accessible name that starts with its visible label (WCAG 2.5.3)', () => {
    for (const outputs of [
      [{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1 }],
      [
        { path: '/out/hearing_ch1.wav', name: 'hearing_ch1.wav', size: 1 },
        { path: '/out/hearing_ch2.wav', name: 'hearing_ch2.wav', size: 1 },
      ],
    ]) {
      const { unmount } = render(
        <FileRow file={file} job={doneJob(outputs)} onRemove={vi.fn()} converting={false} onOpenInPlayer={vi.fn()} />,
      )
      for (const visible of ['Reveal in folder', 'Open in Player']) {
        const button = screen.getByText(visible).closest('button')
        expect(button.textContent.trim()).toBe(visible)
        expect(button.getAttribute('aria-label').startsWith(visible)).toBe(true)
      }
      unmount()
    }
  })

  it('only sends formats the Player can open as they are', () => {
    for (const ext of ['wav', 'mp3', 'flac', 'opus', 'm4a']) expect(isPlayerPlayable(`/out/a.${ext}`)).toBe(true)
    for (const ext of ['trm', 'sgmca', 'ftr', 'txt']) expect(isPlayerPlayable(`/out/a.${ext}`)).toBe(false)
  })
})

describe('Queued rows with several audio tracks', () => {
  afterEach(cleanup)

  const streams = {
    convertedIndex: 0,
    streams: [
      { channels: 1, sampleRate: 48000, language: 'eng', title: 'Floor' },
      { channels: 4, sampleRate: 48000, language: 'spa', title: 'Interpreter' },
    ],
  }

  it('says which track is converted', () => {
    const recording = { path: '/case/hearing.mkv', name: 'hearing.mkv', fmt: null }
    render(<FileRow file={recording} streams={streams} onRemove={vi.fn()} converting={false} />)
    expect(
      screen.getByText(
        'This file has 2 audio tracks; converting track 1 (eng, "Floor"). The other track is not converted.',
      ),
    ).toBeTruthy()
  })

  it('shows no notice for a single track', () => {
    const recording = { path: '/case/hearing.wav', name: 'hearing.wav', fmt: null }
    render(
      <FileRow
        file={recording}
        streams={{ convertedIndex: 0, streams: [streams.streams[0]] }}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    expect(screen.queryByText(/audio tracks/)).toBeNull()
  })
})
