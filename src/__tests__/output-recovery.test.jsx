import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import FileRow from '../components/Convert/FileRow'
import MiniPlayer from '../components/Convert/MiniPlayer'
import LibraryFile from '../components/Library/LibraryFile'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), convertFileSrc: path => path }))
const output = { path: '/test/result.wav', name: 'result.wav', format: 'wav', size: 1024 }

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})
beforeEach(() => invoke.mockReset())

describe('output recovery and handoff', () => {
  it('reveals a single output and sends completed output paths to Player', async () => {
    const onOpenInPlayer = vi.fn()
    invoke.mockResolvedValue(undefined)
    render(
      <FileRow
        file={{ path: '/test/source.sgmca', name: 'source.sgmca' }}
        job={{ status: 'done', outputs: [output] }}
        onOpenInPlayer={onOpenInPlayer}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: /show in explorer/i }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: output.path }))
    fireEvent.click(screen.getByRole('button', { name: /open in player/i }))
    expect(onOpenInPlayer).toHaveBeenCalledWith([output.path])
  })

  it('explains limited SGMCA compatibility beside the queued file', () => {
    render(
      <FileRow
        file={{
          path: '/test/source.sgmca', name: 'source.sgmca',
          fmt: { name: 'Stenograph SGMCA', status: 'experimental', note: 'Some SGMCA variants may not convert.' },
        }}
      />,
    )
    expect(screen.getByText('Some SGMCA variants may not convert.')).toBeVisible()
  })

  it.each([
    ['conversion', () => <MiniPlayer out={output} />],
    ['library', () => <LibraryFile file={output} />],
  ])('shows %s preview failure and lets the user retry', async (_name, component) => {
    vi.spyOn(HTMLMediaElement.prototype, 'play').mockRejectedValueOnce(new Error('decoder unavailable')).mockResolvedValue(undefined)
    const { container } = render(component())
    fireEvent.click(screen.getByRole('button', { name: 'Play result.wav' }))
    expect(await screen.findByRole('alert')).toHaveTextContent(/preview.*could not|could not.*preview/i)
    fireEvent.click(screen.getByRole('button', { name: 'Play result.wav' }))
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument())
    fireEvent.error(container.querySelector('audio'))
    expect(screen.getByRole('alert')).toHaveTextContent(/preview.*could not|could not.*preview/i)
  })
})
