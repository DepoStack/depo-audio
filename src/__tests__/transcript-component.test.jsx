import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import Transcript from '../components/Player/Transcript'
import { storageKey, TRANSCRIPT_SAVE_DEBOUNCE_MS } from '../lib/transcript'

const dialogMocks = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }))
const fileMocks = vi.hoisted(() => ({ readTextFile: vi.fn(), writeTextFile: vi.fn() }))

vi.mock('@tauri-apps/plugin-dialog', () => dialogMocks)
vi.mock('@tauri-apps/plugin-fs', () => fileMocks)

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  delete Element.prototype.scrollIntoView
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

  it('delegates storage warnings without rendering a duplicate alert', () => {
    vi.useFakeTimers()
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('storage blocked')
    })
    const onStorageError = vi.fn()

    render(
      <Transcript
        trackPath="C:\\audio\\delegated.wav"
        currentTime={3}
        playing={false}
        onSeek={vi.fn()}
        onStorageError={onStorageError}
      />,
    )
    act(() => vi.advanceTimersByTime(TRANSCRIPT_SAVE_DEBOUNCE_MS))

    expect(onStorageError).toHaveBeenCalledWith(expect.stringContaining('Transcript autosave is unavailable'))
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
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

  it('debounces transcript persistence and flushes the final edit on cleanup', () => {
    vi.useFakeTimers()
    const write = vi.spyOn(Storage.prototype, 'setItem')
    const path = 'C:\\audio\\debounced.wav'
    const view = render(<Transcript trackPath={path} currentTime={3} playing={false} onSeek={vi.fn()} />)

    fireEvent.click(screen.getByRole('button', { name: /start typing/i }))
    const transcriptText = screen.getByRole('textbox', { name: 'Transcript text for line 1' })
    fireEvent.change(transcriptText, { target: { value: 'first draft' } })
    fireEvent.change(transcriptText, { target: { value: 'settled draft' } })

    expect(write).not.toHaveBeenCalled()
    act(() => vi.advanceTimersByTime(TRANSCRIPT_SAVE_DEBOUNCE_MS - 1))
    expect(write).not.toHaveBeenCalled()
    act(() => vi.advanceTimersByTime(1))
    expect(write).toHaveBeenCalledTimes(1)
    expect(write.mock.calls[0][0]).toBe(storageKey(path))
    expect(write.mock.calls[0][1]).toContain('settled draft')

    fireEvent.change(transcriptText, { target: { value: 'final unslept edit' } })
    view.unmount()
    expect(write).toHaveBeenCalledTimes(2)
    expect(write.mock.calls[1][1]).toContain('final unslept edit')
    act(() => vi.runOnlyPendingTimers())
    expect(write).toHaveBeenCalledTimes(2)
  })

  it('uses instant transcript following when reduced motion is requested', async () => {
    const path = 'C:\\audio\\motion.wav'
    localStorage.setItem(
      storageKey(path),
      JSON.stringify([{ id: 'active', start: 2, speaker: 'Witness', text: 'Answer' }]),
    )
    const scrollIntoView = vi.fn()
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      configurable: true,
      value: scrollIntoView,
    })
    vi.stubGlobal(
      'matchMedia',
      vi.fn(() => ({ matches: true })),
    )

    render(<Transcript trackPath={path} currentTime={2} playing onSeek={vi.fn()} />)

    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest', behavior: 'auto' }))
  })

  it('retains and reports an unsaved draft when a rapid track-switch flush fails', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('storage blocked')
    })
    const onStorageError = vi.fn()
    const firstPath = 'C:\\audio\\rapid-a.wav'
    const secondPath = 'C:\\audio\\rapid-b.wav'
    const view = render(
      <Transcript
        key={firstPath}
        trackPath={firstPath}
        currentTime={1}
        playing={false}
        onSeek={vi.fn()}
        onStorageError={onStorageError}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: /start typing/i }))
    fireEvent.change(screen.getByRole('textbox', { name: 'Transcript text for line 1' }), {
      target: { value: 'Preserve this unsaved answer' },
    })
    view.rerender(
      <Transcript
        key={secondPath}
        trackPath={secondPath}
        currentTime={1}
        playing={false}
        onSeek={vi.fn()}
        onStorageError={onStorageError}
      />,
    )

    expect(onStorageError).toHaveBeenCalledWith(expect.stringContaining('Transcript autosave is unavailable'))

    view.rerender(
      <Transcript
        key={firstPath}
        trackPath={firstPath}
        currentTime={1}
        playing={false}
        onSeek={vi.fn()}
        onStorageError={onStorageError}
      />,
    )

    expect(screen.getByRole('textbox', { name: 'Transcript text for line 1' })).toHaveValue(
      'Preserve this unsaved answer',
    )
    expect(onStorageError).toHaveBeenLastCalledWith(
      expect.stringContaining('Unsaved changes are retained only in this open session'),
    )
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })
})
