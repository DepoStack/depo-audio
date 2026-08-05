import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import Transcript from '../components/Player/Transcript'

const dialogMocks = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }))
const fileMocks = vi.hoisted(() => ({ readTextFile: vi.fn(), writeTextFile: vi.fn() }))

vi.mock('@tauri-apps/plugin-dialog', () => dialogMocks)
vi.mock('@tauri-apps/plugin-fs', () => fileMocks)

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  localStorage.clear()
  dialogMocks.open.mockReset()
  dialogMocks.save.mockReset()
  fileMocks.readTextFile.mockReset()
  fileMocks.writeTextFile.mockReset()
})

describe('Transcript storage and timestamps', () => {
  it('creates a visibly timed line at playback position zero', () => {
    render(<Transcript trackPath="C:\\audio\\zero.wav" currentTime={0} playing={false} onSeek={vi.fn()} />)

    fireEvent.click(screen.getByRole('button', { name: /start typing/i }))

    expect(screen.getByTitle('Jump to this point')).toHaveTextContent('0:00')
  })

  it('warns when transcript autosave is unavailable', async () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('storage blocked')
    })

    render(<Transcript trackPath="C:\\audio\\blocked.wav" currentTime={3} playing={false} onSeek={vi.fn()} />)

    expect(await screen.findByRole('alert')).toHaveTextContent('Transcript autosave is unavailable')
  })

  it('reports transcript import failures instead of treating them as cancellation', async () => {
    dialogMocks.open.mockResolvedValue('C:\\audio\\locked.srt')
    fileMocks.readTextFile.mockRejectedValue(new Error('read denied'))

    render(<Transcript trackPath="C:\\audio\\source.wav" currentTime={3} playing={false} onSeek={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: /import/i }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Transcript import failed: Error: read denied')
  })

  it('reports transcript export write failures', async () => {
    dialogMocks.save.mockResolvedValue('C:\\audio\\transcript.txt')
    fileMocks.writeTextFile.mockRejectedValue(new Error('disk full'))

    render(<Transcript trackPath="C:\\audio\\source.wav" currentTime={3} playing={false} onSeek={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: /start typing/i }))
    fireEvent.click(screen.getByRole('button', { name: /^export$/i }))
    fireEvent.click(screen.getByRole('button', { name: /plain text/i }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Transcript export failed: Error: disk full')
  })

  it('gives the keyboard-revealed re-stamp control an accessible name', () => {
    render(<Transcript trackPath="C:\\audio\\source.wav" currentTime={3} playing={false} onSeek={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: /start typing/i }))

    const restamp = screen.getByRole('button', { name: /set this line's time to the current position/i })
    expect(restamp.parentElement).toHaveClass('group-focus-within:opacity-100')
  })
})
