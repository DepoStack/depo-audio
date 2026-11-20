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

    fireEvent.click(await screen.findByRole('button', { name: 'Replace' }))
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
})
