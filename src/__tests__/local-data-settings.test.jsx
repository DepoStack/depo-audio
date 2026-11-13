import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import LocalDataSettings from '../components/LocalDataSettings'
import { storageKey } from '../lib/transcript'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }))

// Synthetic data only.
const APP_DATA = '/home/reporter/.local/share/com.andrewmayes.depoaudio'
const line = { id: 'a', start: 1, speaker: 'Q', text: 'Synthetic line' }

function info(overrides = {}) {
  return {
    appDataDir: APP_DATA,
    libraryReadable: true,
    items: [
      { name: 'library.json', kind: 'library', bytes: 2048 },
      { name: 'prefs.json', kind: 'prefs', bytes: 300 },
    ],
    ...overrides,
  }
}

function mockBackend(handlers = {}) {
  invoke.mockImplementation((command, args) => {
    if (command in handlers) return Promise.resolve().then(() => handlers[command](args))
    if (command === 'local_data_info_cmd') return Promise.resolve(info())
    return Promise.resolve()
  })
}

function renderPanel(props = {}) {
  const reloadApp = vi.fn()
  const onReloadNeeded = vi.fn()
  render(<LocalDataSettings reloadApp={reloadApp} onReloadNeeded={onReloadNeeded} {...props} />)
  return { reloadApp, onReloadNeeded }
}

describe('Local data settings', () => {
  beforeEach(() => {
    invoke.mockReset()
    open.mockReset()
    save.mockReset()
    localStorage.clear()
    localStorage.setItem(storageKey('/out/synthetic.wav'), JSON.stringify([line]))
    localStorage.setItem('player-bookmarks', JSON.stringify([{ time: 4, trackPath: '/out/synthetic.wav' }]))
    localStorage.setItem('player-speed', '1.5')
    localStorage.setItem('unrelated', 'kept')
  })

  afterEach(cleanup)

  it('shows where data is kept and works with no account or network', async () => {
    mockBackend()
    renderPanel()

    expect(await screen.findByText(APP_DATA)).toBeVisible()
    expect(screen.getByText('library.json')).toBeVisible()
    expect(screen.getByText(/case names, sessions/i)).toBeVisible()
    expect(screen.getByText(/1 track transcript/i)).toBeVisible()
    expect(screen.getByText(/1 bookmark/i)).toBeVisible()
    expect(screen.getByText(/full-disk encryption/i)).toBeVisible()
    expect(screen.queryByText(/sign in|account required/i)).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Show the app data folder' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: APP_DATA }))
  })

  it('exports the library with the transcripts and bookmarks from this computer', async () => {
    mockBackend({
      local_data_export_cmd: () => ({ cases: 2, sessions: 3, transcripts: 1, bookmarks: 1 }),
    })
    save.mockResolvedValue('/backups/DepoAudio backup.zip')
    renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: /export library and transcripts/i }))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('local_data_export_cmd', {
        path: '/backups/DepoAudio backup.zip',
        transcripts: { '/out/synthetic.wav': [line] },
        bookmarks: [{ time: 4, trackPath: '/out/synthetic.wav' }],
      }),
    )
    expect(save).toHaveBeenCalledWith(
      expect.objectContaining({ filters: [{ name: 'DepoAudio backup', extensions: ['zip'] }] }),
    )
    expect(await screen.findByText(/2 cases, 3 sessions, 1 transcript and 1 bookmark/i)).toBeVisible()
  })

  it('does nothing when the save dialog is cancelled', async () => {
    mockBackend()
    save.mockResolvedValue(null)
    renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: /export library and transcripts/i }))
    await waitFor(() => expect(save).toHaveBeenCalled())
    expect(invoke.mock.calls.map(([command]) => command)).not.toContain('local_data_export_cmd')
  })

  it('reports an export failure', async () => {
    mockBackend({
      local_data_export_cmd: () => {
        throw 'A file with that name already exists. DepoAudio never replaces a file; choose a new name.'
      },
    })
    save.mockResolvedValue('/backups/old.zip')
    renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: /export library and transcripts/i }))
    expect(await screen.findByRole('alert')).toHaveTextContent('never replaces a file')
  })

  it('disables export when the library could not be loaded, and says why', async () => {
    mockBackend({ local_data_info_cmd: () => info({ libraryReadable: false }) })
    renderPanel()

    expect(await screen.findByRole('button', { name: /export library and transcripts/i })).toBeDisabled()
    expect(screen.getByText(/library could not be loaded/i)).toBeVisible()
  })

  it('imports a backup, restores only missing transcripts, and asks for a reload', async () => {
    mockBackend({
      local_data_import_cmd: () => ({
        casesAdded: 1,
        sessionsAdded: 2,
        sessionsSkipped: 1,
        filesUnavailable: 1,
        transcripts: {
          '/out/synthetic.wav': [{ ...line, text: 'Different text' }],
          '/out/new.wav': [line],
        },
        bookmarks: [{ time: 9, trackPath: '/out/new.wav' }],
      }),
    })
    open.mockResolvedValue('/backups/DepoAudio backup.zip')
    const { reloadApp, onReloadNeeded } = renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: /import a backup/i }))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('local_data_import_cmd', { path: '/backups/DepoAudio backup.zip' }),
    )
    const status = await screen.findByRole('status', { name: 'Import result' })
    expect(status).toHaveTextContent(/1 case and 2 sessions added/i)
    expect(status).toHaveTextContent(/1 session was already here/i)
    expect(status).toHaveTextContent(/1 transcript added/i)
    expect(status).toHaveTextContent(/1 transcript on this computer was kept/i)
    expect(status).toHaveTextContent(/1 file .*not on this computer/i)

    expect(JSON.parse(localStorage.getItem(storageKey('/out/synthetic.wav')))[0].text).toBe('Synthetic line')
    expect(JSON.parse(localStorage.getItem(storageKey('/out/new.wav')))).toEqual([line])
    expect(JSON.parse(localStorage.getItem('player-bookmarks'))).toHaveLength(2)
    expect(onReloadNeeded).toHaveBeenCalled()
    expect(reloadApp).not.toHaveBeenCalled()
    fireEvent.click(within(status).getByRole('button', { name: /reload depoaudio/i }))
    expect(reloadApp).toHaveBeenCalled()
  })

  it('says files removed from playback this session need a restart, not that they are missing', async () => {
    mockBackend({
      local_data_import_cmd: () => ({
        casesAdded: 1,
        sessionsAdded: 1,
        sessionsSkipped: 0,
        filesUnavailable: 0,
        filesNeedRestart: 2,
        transcripts: {},
        bookmarks: [],
      }),
    })
    open.mockResolvedValue('/backups/DepoAudio backup.zip')
    renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: /import a backup/i }))
    const status = await screen.findByRole('status', { name: 'Import result' })
    expect(status).toHaveTextContent(/2 files .*removed from playback .*this app session/i)
    expect(status).toHaveTextContent(/restart DepoAudio/i)
    expect(status).not.toHaveTextContent(/not on this computer/i)
  })

  it('keeps Clear all unavailable while files are converting', async () => {
    mockBackend()
    renderPanel({ conversionActive: true })

    expect(await screen.findByRole('button', { name: 'Clear all DepoAudio data…' })).toBeDisabled()
    expect(screen.getByText(/not available while files are converting/i)).toBeVisible()
  })

  it('shows the refusal when the app is still converting at the moment of clearing', async () => {
    mockBackend({
      local_data_clear_cmd: () => {
        throw 'Clear all is not available while files are converting. Wait for them to finish or cancel them, then try again.'
      },
    })
    const { reloadApp } = renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: 'Clear all DepoAudio data…' }))
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Clear all data' }))

    expect(await screen.findByRole('alert')).toHaveTextContent(/nothing was cleared.*while files are converting/i)
    expect(reloadApp).not.toHaveBeenCalled()
    expect(localStorage.getItem(storageKey('/out/synthetic.wav'))).not.toBeNull()
  })

  it('clears all DepoAudio data only after confirmation, then reloads', async () => {
    mockBackend({ local_data_clear_cmd: () => ({ removed: 2, warning: null }) })
    const { reloadApp } = renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: 'Clear all DepoAudio data…' }))
    const dialog = screen.getByRole('dialog')
    expect(dialog).toHaveTextContent(/cannot be undone/i)
    expect(dialog).toHaveTextContent(/recordings, converted files, export records and backups are not deleted/i)
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(invoke.mock.calls.map(([command]) => command)).not.toContain('local_data_clear_cmd')
    expect(localStorage.getItem(storageKey('/out/synthetic.wav'))).not.toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Clear all DepoAudio data…' }))
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Clear all data' }))

    await waitFor(() => expect(reloadApp).toHaveBeenCalled())
    expect(invoke).toHaveBeenCalledWith('local_data_clear_cmd')
    expect(localStorage.getItem(storageKey('/out/synthetic.wav'))).toBeNull()
    expect(localStorage.getItem('player-bookmarks')).toBeNull()
    expect(localStorage.getItem('player-speed')).toBeNull()
    expect(localStorage.getItem('unrelated')).toBe('kept')
    // The backend is never asked to delete a recording or output path.
    const commands = invoke.mock.calls.map(([command]) => command)
    expect(commands.filter(command => /delete|remove/.test(command))).toEqual([])
  })

  it('shows a partial-clear warning instead of reloading', async () => {
    mockBackend({
      local_data_clear_cmd: () => ({ removed: 1, warning: '1 file(s) could not be removed (prefs.json).' }),
    })
    const { reloadApp, onReloadNeeded } = renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: 'Clear all DepoAudio data…' }))
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Clear all data' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('prefs.json')
    expect(reloadApp).not.toHaveBeenCalled()
    expect(onReloadNeeded).toHaveBeenCalled()
  })
})
