import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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
  it.each([
    ['another track', false, false],
    ['the original track after leaving and returning', true, false],
    ['another track when reading fails', false, true],
  ])('ignores an old import completion while editing %s', async (_label, returnToOriginal, rejectRead) => {
    const firstPath = `C:\\audio\\deferred-a-${returnToOriginal}-${rejectRead}.wav`
    const secondPath = `C:\\audio\\deferred-b-${returnToOriginal}-${rejectRead}.wav`
    for (const [path, text] of [
      [firstPath, 'Track A draft'],
      [secondPath, 'Track B draft'],
    ]) {
      localStorage.setItem(storageKey(path), JSON.stringify([{ id: 'draft', start: 1, text }]))
    }
    let resolveRead
    let failRead
    dialogMocks.open.mockResolvedValue('C:\\audio\\late-import.txt')
    fileMocks.readTextFile.mockImplementation(
      () =>
        new Promise((resolve, reject) => {
          resolveRead = resolve
          failRead = reject
        }),
    )
    const view = render(<Transcript trackPath={firstPath} currentTime={0} playing={false} onSeek={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: /^import$/i }))
    await waitFor(() => expect(fileMocks.readTextFile).toHaveBeenCalled())

    view.rerender(<Transcript trackPath={secondPath} currentTime={0} playing={false} onSeek={vi.fn()} />)
    if (returnToOriginal) {
      view.rerender(<Transcript trackPath={firstPath} currentTime={0} playing={false} onSeek={vi.fn()} />)
    }
    fireEvent.click(screen.getByRole('button', { name: 'Delete line' }))
    expect(screen.getByRole('button', { name: /undo/i })).toBeInTheDocument()
    await act(async () => {
      if (rejectRead) failRead(new Error('stale read failed'))
      else resolveRead('Late import for old track')
    })

    expect(screen.getByRole('button', { name: /undo/i })).toBeInTheDocument()
    expect(screen.queryByDisplayValue('Late import for old track')).not.toBeInTheDocument()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(screen.queryByText(/stale read failed/i)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /undo/i }))
    expect(screen.getByDisplayValue(returnToOriginal ? 'Track A draft' : 'Track B draft')).toBeInTheDocument()
  })

  it('does not read an import selected after the editor unmounts', async () => {
    let selectFile
    dialogMocks.open.mockImplementation(
      () =>
        new Promise(resolve => {
          selectFile = resolve
        }),
    )
    const view = render(
      <Transcript trackPath="C:\\audio\\unmounted-import.wav" currentTime={0} playing={false} onSeek={vi.fn()} />,
    )
    fireEvent.click(screen.getByRole('button', { name: /^import$/i }))
    view.unmount()
    await act(async () => {
      selectFile('C:\\audio\\late.txt')
    })
    expect(fileMocks.readTextFile).not.toHaveBeenCalled()
  })

  it('keeps the current transcript until replacement is confirmed and supports undoing replacement', async () => {
    const path = 'C:\\audio\\replace.wav'
    localStorage.setItem(storageKey(path), JSON.stringify([{ id: 'old', start: 1, end: 2, text: 'Original answer' }]))
    dialogMocks.open.mockResolvedValue('C:\\audio\\replacement.srt')
    fileMocks.readTextFile.mockResolvedValue('1\n00:00:10,000 --> 00:00:20,000\nReplacement answer\n')
    render(<Transcript trackPath={path} currentTime={3} playing={false} onSeek={vi.fn()} />)

    fireEvent.click(screen.getByRole('button', { name: /^import$/i }))
    const confirmation = await screen.findByRole('dialog', { name: 'Replace this transcript?' })
    expect(screen.getByDisplayValue('Original answer')).toBeInTheDocument()
    fireEvent.click(within(confirmation).getByRole('button', { name: 'Cancel' }))
    expect(screen.getByDisplayValue('Original answer')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: /^import$/i }))
    fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Replace transcript' }))
    expect(screen.getByDisplayValue('Replacement answer')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /undo/i }))
    expect(screen.getByDisplayValue('Original answer')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /undo/i })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: /^export$/i }))
    dialogMocks.save.mockResolvedValue('C:\\audio\\restored.srt')
    fileMocks.writeTextFile.mockResolvedValue()
    fireEvent.click(screen.getByRole('button', { name: /subtitles/i }))
    await waitFor(() =>
      expect(fileMocks.writeTextFile).toHaveBeenCalledWith(
        'C:\\audio\\restored.srt',
        expect.stringContaining('00:00:01,000 --> 00:00:02,000'),
      ),
    )
  })

  it('restores a deleted final line with its timing and clears undo after a later edit', () => {
    const path = 'C:\\audio\\undo-delete.wav'
    localStorage.setItem(storageKey(path), JSON.stringify([{ id: 'old', start: 1, end: 2, text: 'Keep this answer' }]))
    render(<Transcript trackPath={path} currentTime={3} playing={false} onSeek={vi.fn()} />)

    fireEvent.click(screen.getByRole('button', { name: 'Delete line' }))
    expect(screen.queryByDisplayValue('Keep this answer')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /undo/i }))
    expect(screen.getByDisplayValue('Keep this answer')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Delete line' }))
    fireEvent.click(screen.getByRole('button', { name: /start typing/i }))
    expect(screen.queryByRole('button', { name: /undo/i })).not.toBeInTheDocument()
  })

  it('does not erase existing work when an imported subtitle contains no cues', async () => {
    const path = 'C:\\audio\\empty-import.wav'
    localStorage.setItem(storageKey(path), JSON.stringify([{ id: 'old', start: 1, text: 'Original answer' }]))
    dialogMocks.open.mockResolvedValue('C:\\audio\\empty.srt')
    fileMocks.readTextFile.mockResolvedValue('')
    render(<Transcript trackPath={path} currentTime={3} playing={false} onSeek={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: /^import$/i }))

    expect(await screen.findByRole('alert')).toHaveTextContent(/no transcript lines/i)
    expect(screen.getByDisplayValue('Original answer')).toBeInTheDocument()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })

  it('does not carry a delete undo across track changes', () => {
    const firstPath = 'C:\\audio\\undo-track-a.wav'
    const secondPath = 'C:\\audio\\undo-track-b.wav'
    localStorage.setItem(storageKey(firstPath), JSON.stringify([{ id: 'a', start: 1, text: 'First track' }]))
    const view = render(<Transcript trackPath={firstPath} currentTime={0} playing={false} onSeek={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: 'Delete line' }))
    expect(screen.getByRole('button', { name: /undo/i })).toBeInTheDocument()

    view.rerender(<Transcript trackPath={secondPath} currentTime={0} playing={false} onSeek={vi.fn()} />)
    expect(screen.queryByRole('button', { name: /undo/i })).not.toBeInTheDocument()
    view.rerender(<Transcript trackPath={firstPath} currentTime={0} playing={false} onSeek={vi.fn()} />)
    expect(screen.queryByRole('button', { name: /undo/i })).not.toBeInTheDocument()
  })

  it('clears imported end timing when a line is deliberately re-stamped', async () => {
    const path = 'C:\\audio\\restamp-import.wav'
    localStorage.setItem(storageKey(path), JSON.stringify([{ id: 'old', start: 1, end: 20, text: 'Answer' }]))
    dialogMocks.save.mockResolvedValue('C:\\audio\\restamped.srt')
    fileMocks.writeTextFile.mockResolvedValue()
    render(<Transcript trackPath={path} currentTime={5} playing={false} onSeek={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: /set this line's time to the current position/i }))
    fireEvent.click(screen.getByRole('button', { name: /^export$/i }))
    fireEvent.click(screen.getByRole('button', { name: /subtitles/i }))

    await waitFor(() =>
      expect(fileMocks.writeTextFile).toHaveBeenCalledWith(
        'C:\\audio\\restamped.srt',
        expect.stringContaining('00:00:05,000 --> 00:00:08,000'),
      ),
    )
  })

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
