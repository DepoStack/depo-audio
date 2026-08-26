import { readFileSync } from 'node:fs'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { open as openDialog } from '@tauri-apps/plugin-dialog'

import App from '../App'
import ImportModal from '../components/Library/ImportModal'
import LibraryTab from '../components/Library/LibraryTab'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn(path => path),
  invoke: vi.fn(),
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }))
vi.mock('../hooks/PreferencesContext', () => ({
  usePreferencesContext: () => ({
    autoLevel: false,
    dereverb: false,
    labels: ['Reporter', 'Witness'],
    maxScanDepth: 7,
    mode: 'stereo',
    outDir: '',
    prefsError: 'Preferences could not be loaded: TypeError: invoke unavailable.',
    prefsReady: true,
    setThemePref: vi.fn(),
    themePref: 'system',
  }),
}))
vi.mock('../hooks/useTheme', () => ({
  default: () => ({
    cycleTheme: vi.fn(),
    setThemeDirect: vi.fn(),
    themeLabel: 'system',
    themePref: 'system',
  }),
}))
vi.mock('../hooks/useFileDrop', () => ({
  default: () => ({
    addFiles: vi.fn(() => Promise.resolve({ locked: false })),
    browseFiles: vi.fn(),
    browseOutDir: vi.fn(),
    caseName: '',
    clearAll: vi.fn(),
    dragOver: false,
    files: [],
    onDragLeave: vi.fn(),
    onDragOver: vi.fn(),
    onDrop: vi.fn(),
    queueNotice: '',
    removeFile: vi.fn(),
    setCaseName: vi.fn(),
  }),
}))
vi.mock('../hooks/useConversion', () => ({
  default: () => ({
    cancelConversion: vi.fn(),
    cancelError: '',
    cancelledCount: 0,
    cancelling: false,
    converting: false,
    doneCount: 0,
    failCount: 0,
    jobs: [],
    startConversion: vi.fn(),
  }),
}))
vi.mock('../hooks/useUpdater', () => ({
  default: () => ({
    dismissed: false,
    error: null,
    progress: 0,
    status: 'idle',
    update: null,
  }),
}))
vi.mock('../components/Convert/ConvertTab', () => ({ default: () => <section>Convert workspace</section> }))
vi.mock('../components/Player/PlayerTab', () => ({ default: () => <section>Player workspace</section> }))
vi.mock('../components/SettingsPanel', () => ({ default: () => null }))
vi.mock('../components/UpdateBanner', () => ({ default: () => null }))
vi.mock('../lib/speakerColors', () => ({
  speakerColorAt: () => '#111',
  useSpeakerColors: () => ['#111'],
}))

const caseRecord = {
  archived: false,
  createdAt: '2026-08-25T12:00:00.000Z',
  id: 'case-1',
  name: 'Smith v. Metro Transit',
  sessions: [],
}

describe('accessible app shell and Library', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue(undefined)
    openDialog.mockReset()
    openDialog.mockResolvedValue(null)
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('provides the everyday shell landmark and keeps raw storage diagnostics behind disclosure', async () => {
    invoke.mockRejectedValue(new TypeError('invoke unavailable'))

    render(<App />)

    const heading = screen.getByRole('heading', { level: 1, name: 'DepoAudio workspace' })
    expect(screen.getByRole('main', { name: 'DepoAudio workspace' })).toContainElement(heading)

    await waitFor(() => expect(screen.getByText(/preferences and Library data could not be loaded/i)).toBeVisible())
    const storageAlert = screen.getByText('Storage protection active.').closest('[role="alert"]')
    const disclosure = within(storageAlert).getByText('Technical details').closest('details')
    const diagnostic = within(disclosure).getByText(/TypeError: invoke unavailable/i)

    expect(disclosure).not.toHaveAttribute('open')
    expect(diagnostic).toBeInTheDocument()
    expect(diagnostic).not.toBeVisible()
  })

  it('labels search durably and keeps rename controls outside the case expansion button', async () => {
    const setCases = vi.fn()
    render(
      <LibraryTab
        cases={[caseRecord]}
        setCases={setCases}
        search=""
        setSearch={vi.fn()}
        labels={['Reporter']}
        onReexport={vi.fn()}
      />,
    )

    const searchInput = screen.getByRole('searchbox', { name: 'Search Library cases and participants' })
    expect(searchInput).toHaveAttribute('aria-controls', 'library-case-list')

    const expansion = screen.getByRole('button', { name: /Expand Smith v\. Metro Transit/i })
    const controlledPanel = document.getElementById(expansion.getAttribute('aria-controls'))
    expect(controlledPanel).toHaveAttribute('hidden')
    const renameButton = screen.getByRole('button', { name: 'Rename Smith v. Metro Transit' })
    fireEvent.click(renameButton)

    const renameInput = screen.getByRole('textbox', { name: 'Rename Smith v. Metro Transit' })
    expect(renameInput).toHaveFocus()
    expect(expansion).not.toContainElement(renameInput)
    expect(expansion).not.toContainElement(screen.getByRole('button', { name: 'Save' }))
    expect(expansion).not.toContainElement(screen.getByRole('button', { name: 'Cancel' }))

    fireEvent.change(renameInput, { target: { value: 'Smith v. City Transit' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('library_rename_case', {
        caseId: 'case-1',
        name: 'Smith v. City Transit',
      }),
    )
    await waitFor(() => expect(renameButton).toHaveFocus())

    fireEvent.click(expansion)
    expect(controlledPanel).not.toHaveAttribute('hidden')
  })

  it('exposes Import choices, selected state, durable field labels, and focused validation', async () => {
    render(
      <ImportModal
        defaultLabels={['Reporter', 'Witness']}
        existingCases={['Smith v. Metro Transit']}
        onDone={vi.fn()}
        onClose={vi.fn()}
        onStorageError={vi.fn()}
      />,
    )

    expect(screen.getByRole('dialog', { name: 'Import Audio to Library' })).toBeVisible()
    expect(screen.getByRole('group', { name: 'Case name' })).toBeVisible()
    expect(screen.getByLabelText('Existing case')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Existing case' })).toHaveAttribute('aria-pressed', 'true')

    fireEvent.click(screen.getByRole('button', { name: 'New case' }))
    expect(screen.getByRole('textbox', { name: 'New case name' })).toBeVisible()

    expect(screen.getByRole('button', { name: 'Reporter' })).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(screen.getByRole('button', { name: /Custom/ }))
    expect(screen.getByRole('textbox', { name: 'Custom speaker or participant' })).toBeVisible()

    fireEvent.click(screen.getByRole('button', { name: 'Import' }))
    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent('Select at least one file')
    expect(alert).toHaveFocus()
  })

  it('distinguishes file-picker failure from intentional cancellation', async () => {
    openDialog.mockRejectedValueOnce(new Error('dialog unavailable'))

    render(
      <ImportModal
        defaultLabels={['Reporter']}
        existingCases={[]}
        onDone={vi.fn()}
        onClose={vi.fn()}
        onStorageError={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Browse files…' }))

    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent('The file picker could not be opened')
    expect(alert).toHaveFocus()
    const disclosure = within(alert).getByText('Technical details').closest('details')
    expect(disclosure).not.toHaveAttribute('open')
    expect(within(disclosure).getByText('Error: dialog unavailable')).not.toBeVisible()
  })

  it('defines global reduced-motion and compact Library reflow fallbacks', () => {
    const css = readFileSync('src/globals.css', 'utf8')

    expect(css).toMatch(/@media\s*\(prefers-reduced-motion:\s*reduce\)/)
    expect(css).toMatch(/animation:\s*none\s*!important/)
    expect(css).toMatch(/@media\s*\(max-width:\s*560px\)/)
    expect(css).toMatch(/\.app-sidebar\s*\{[\s\S]*?width:\s*3rem/)
    expect(css).toMatch(/\.library-file-seek\s*\{[\s\S]*?width:\s*auto\s*!important/)
  })
})
