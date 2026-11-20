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

    fireEvent.click(screen.getByRole('button', { name: 'Reveal hearing.mp3 in its folder' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: '/out/hearing.mp3' }))

    fireEvent.click(screen.getByRole('button', { name: 'Open hearing.mp3 in Player' }))
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

    fireEvent.click(screen.getByRole('button', { name: 'Reveal the outputs of hearing.trm in their folder' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: '/out/hearing_ch1.wav' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open the 2 outputs of hearing.trm in Player' }))
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
    fireEvent.click(screen.getByRole('button', { name: 'Reveal hearing.flac in its folder' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('opener unavailable')
    // Without a Player handler there is no Open in Player button
    expect(screen.queryByRole('button', { name: /in Player/ })).not.toBeInTheDocument()
  })

  it('only sends formats the Player can open as they are', () => {
    for (const ext of ['wav', 'mp3', 'flac', 'opus', 'm4a']) expect(isPlayerPlayable(`/out/a.${ext}`)).toBe(true)
    for (const ext of ['trm', 'sgmca', 'ftr', 'txt']) expect(isPlayerPlayable(`/out/a.${ext}`)).toBe(false)
  })
})
