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
  it('shows a concise conversion failure before opening technical details', () => {
    render(
      <FileRow
        file={{ path: '/test/source.wav', name: 'source.wav' }}
        job={{ status: 'error', error: 'Output folder is not writable.\nTechnical decoder detail' }}
      />,
    )
    expect(screen.getByRole('alert')).toHaveTextContent('Output folder is not writable.')
    expect(screen.queryByText(/Technical decoder detail/)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /details/i }))
    expect(screen.getByText(/Technical decoder detail/)).toBeVisible()
  })

  it('explains a failure even when the backend supplied no message', () => {
    render(<FileRow file={{ path: '/test/source.wav', name: 'source.wav' }} job={{ status: 'error' }} />)
    expect(screen.getByRole('alert')).toHaveTextContent(/conversion did not finish/i)
    expect(screen.getByRole('alert')).toHaveTextContent(/source file and output folder/i)
  })

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
