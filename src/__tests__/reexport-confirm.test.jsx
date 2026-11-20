import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import App from '../App'

const fileDrop = vi.hoisted(() => ({ current: null }))

vi.mock('@tauri-apps/api/core', () => ({ convertFileSrc: vi.fn(path => path), invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }))
vi.mock('../hooks/PreferencesContext', () => ({
  usePreferencesContext: () => ({ labels: [], mode: 'stereo', outDir: '', prefsReady: true, themePref: 'system' }),
}))
vi.mock('../hooks/useTheme', () => ({
  default: () => ({ cycleTheme: vi.fn(), setThemeDirect: vi.fn(), themeLabel: 'system', themePref: 'system' }),
}))
vi.mock('../hooks/useFileDrop', () => ({ default: () => fileDrop.current }))
vi.mock('../hooks/useConversion', () => ({
  default: () => ({
    cancelConversion: vi.fn(),
    cancelError: '',
    cancelledCount: 0,
    cancelling: false,
    converting: false,
    doneCount: 0,
    failCount: 0,
    jobs: {},
    startConversion: vi.fn(),
  }),
}))
vi.mock('../hooks/useUpdater', () => ({
  default: () => ({ dismissed: false, error: null, progress: 0, status: 'idle', update: null }),
}))
vi.mock('../components/Convert/ConvertTab', () => ({ default: () => <section>Convert workspace</section> }))
vi.mock('../components/Player/PlayerTab', () => ({ default: () => <section>Player workspace</section> }))
vi.mock('../components/SettingsPanel', () => ({ default: () => null }))
vi.mock('../components/UpdateBanner', () => ({ default: () => null }))
vi.mock('../components/Library/LibraryTab', () => ({
  default: ({ onReexport }) => (
    <button type="button" onClick={() => onReexport('/case/source.trm', 'Smith v. Metro')}>
      Re-export
    </button>
  ),
}))

function makeFileDrop(files) {
  return {
    addFiles: vi.fn(() => Promise.resolve({ locked: false })),
    browseFiles: vi.fn(),
    browseOutDir: vi.fn(),
    caseName: files.length ? 'Existing case' : '',
    clearAll: vi.fn(),
    dragOver: false,
    files,
    onDragLeave: vi.fn(),
    onDragOver: vi.fn(),
    onDrop: vi.fn(),
    queueNotice: '',
    removeFile: vi.fn(),
    setCaseName: vi.fn(),
  }
}

const queued = [
  { path: '/case/a.wav', name: 'a.wav', fmt: null },
  { path: '/case/b.wav', name: 'b.wav', fmt: null },
]

async function openLibraryAndReexport() {
  fireEvent.mouseDown(screen.getByRole('tab', { name: /Library/ }))
  fireEvent.click(await screen.findByRole('button', { name: 'Re-export' }))
}

describe('Library re-export into the Convert queue', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue([])
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('fills an empty queue without asking', async () => {
    fileDrop.current = makeFileDrop([])
    render(<App />)
    await openLibraryAndReexport()

    await waitFor(() => expect(fileDrop.current.addFiles).toHaveBeenCalledWith(['/case/source.trm'], { replace: true }))
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })

  it('asks before replacing a non-empty queue and changes nothing on Cancel', async () => {
    fileDrop.current = makeFileDrop(queued)
    render(<App />)
    await openLibraryAndReexport()

    expect(await screen.findByRole('dialog', { name: 'Replace the 2 files in the queue?' })).toBeVisible()
    expect(fileDrop.current.addFiles).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(fileDrop.current.addFiles).not.toHaveBeenCalled()
    expect(fileDrop.current.setCaseName).not.toHaveBeenCalled()
  })

  it('replaces the queue only after Replace', async () => {
    fileDrop.current = makeFileDrop(queued)
    render(<App />)
    await openLibraryAndReexport()

    fireEvent.click(await screen.findByRole('button', { name: 'Replace 2 files' }))
    await waitFor(() => expect(fileDrop.current.addFiles).toHaveBeenCalledWith(['/case/source.trm'], { replace: true }))
    await waitFor(() => expect(fileDrop.current.setCaseName).toHaveBeenCalledWith('Smith v. Metro'))
  })

  it('can add to the queue instead, keeping the files and case name already there', async () => {
    fileDrop.current = makeFileDrop(queued)
    render(<App />)
    await openLibraryAndReexport()

    fireEvent.click(await screen.findByRole('button', { name: 'Add to queue' }))
    await waitFor(() =>
      expect(fileDrop.current.addFiles).toHaveBeenCalledWith(['/case/source.trm'], { replace: false }),
    )
    expect(fileDrop.current.setCaseName).not.toHaveBeenCalled()
  })

  it('marks Replace as the destructive choice and focuses Add to queue, so Enter never replaces', async () => {
    fileDrop.current = makeFileDrop(queued)
    render(<App />)
    await openLibraryAndReexport()

    const replace = await screen.findByRole('button', { name: 'Replace 2 files' })
    const add = screen.getByRole('button', { name: 'Add to queue' })
    expect(replace).toHaveAttribute('data-destructive', 'true')
    expect(add).not.toHaveAttribute('data-destructive')
    expect(replace.className).not.toBe(add.className)
    expect(replace.className).toMatch(/bg-destructive/)
    // The focused button is the one Enter activates in the browser
    await waitFor(() => expect(add).toHaveFocus())
    expect(replace).not.toHaveFocus()
  })

  it('uses singular copy for one queued file and one Library file', async () => {
    fileDrop.current = makeFileDrop([queued[0]])
    render(<App />)
    await openLibraryAndReexport()

    const dialog = await screen.findByRole('dialog', { name: 'Replace the 1 file in the queue?' })
    expect(dialog).toHaveTextContent('The file waiting in Convert is taken out of the queue.')
    expect(dialog).toHaveTextContent('Add to queue keeps it and adds the Library file after it.')
    expect(dialog).not.toHaveTextContent(/\bfiles\b/)
    expect(screen.getByRole('button', { name: 'Replace 1 file' })).toBeInTheDocument()
  })

  it('counts a file dropped while the dialog is open and replaces the queue as it is at click time', async () => {
    fileDrop.current = makeFileDrop(queued)
    const { rerender } = render(<App />)
    await openLibraryAndReexport()
    expect(await screen.findByRole('dialog', { name: 'Replace the 2 files in the queue?' })).toBeVisible()

    // A third file arrives (drag and drop) while the dialog is open
    const addFiles = fileDrop.current.addFiles
    fileDrop.current = { ...makeFileDrop([...queued, { path: '/case/c.wav', name: 'c.wav', fmt: null }]), addFiles }
    rerender(<App />)

    const dialog = await screen.findByRole('dialog', { name: 'Replace the 3 files in the queue?' })
    expect(dialog).toHaveTextContent('1 file was added to the queue while this was open and is counted here.')
    expect(screen.queryByRole('button', { name: 'Replace 2 files' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Replace 3 files' }))
    await waitFor(() => expect(addFiles).toHaveBeenCalledWith(['/case/source.trm'], { replace: true }))
  })

  it('does not offer Replace once the queue has been emptied while the dialog is open', async () => {
    fileDrop.current = makeFileDrop(queued)
    const { rerender } = render(<App />)
    await openLibraryAndReexport()
    await screen.findByRole('dialog', { name: 'Replace the 2 files in the queue?' })

    const addFiles = fileDrop.current.addFiles
    fileDrop.current = { ...makeFileDrop([]), addFiles }
    rerender(<App />)

    expect(await screen.findByRole('dialog', { name: 'The Convert queue is now empty' })).toBeVisible()
    expect(screen.queryByRole('button', { name: /Replace/ })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Queue' }))
    await waitFor(() => expect(addFiles).toHaveBeenCalledWith(['/case/source.trm'], { replace: true }))
  })
})
