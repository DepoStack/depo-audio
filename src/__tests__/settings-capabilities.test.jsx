import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import SettingsPanel from '../components/SettingsPanel'
import { DEPOAUDIO_RELEASES_URL } from '../constants'
import { buildAppStylesheet, injectStylesheet } from './helpers/appStylesheet'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }))

function renderSettings(prefs = {}) {
  return render(<SettingsPanel open onOpenChange={vi.fn()} prefs={prefs} />)
}

describe('SettingsPanel release boundaries', () => {
  afterEach(cleanup)

  beforeEach(() => {
    invoke.mockReset()
    openUrl.mockReset()
  })

  it('names DepoAudio as powered by DepoStack in About', async () => {
    invoke.mockResolvedValue([])

    renderSettings()
    fireEvent.click(screen.getByRole('button', { name: 'Updates' }))

    // The wordmark leads and the endorsement follows in its own smaller run, so match the whole name.
    const name = await screen.findByText(
      (_, element) => element?.textContent === 'DepoAudio, powered by DepoStack' && element.children.length === 2,
    )
    expect(name).toBeVisible()
    expect(within(name).getByText('DepoAudio')).toHaveClass('type-wordmark')
    expect(screen.queryByText(/part of DepoStack/i)).not.toBeInTheDocument()
  })

  it('shows a deletion-only empty state without requesting capabilities or model downloads', async () => {
    invoke.mockResolvedValue([])

    renderSettings()

    expect(await screen.findByText(/does not install or download learned-model files/i)).toBeVisible()
    expect(screen.queryByRole('button', { name: /install/i })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /download .*model/i })).not.toBeInTheDocument()
    expect(invoke).toHaveBeenCalledWith('legacy_model_cleanup_catalog_cmd')
    expect(invoke.mock.calls.map(([command]) => command)).not.toContain('system_capabilities_cmd')
    expect(invoke.mock.calls.map(([command]) => command)).not.toContain('download_model_cmd')
  })

  it('removes an installed legacy file through the cleanup-only command', async () => {
    let legacyPresent = true
    const legacyModel = {
      filename: 'speaker_embed.onnx',
      displayName: 'Speaker embedding (legacy)',
      description: 'Unused by this release',
      sizeMb: 4.2,
      installed: true,
      removable: true,
    }
    invoke.mockImplementation(command => {
      if (command === 'legacy_model_cleanup_catalog_cmd') {
        return Promise.resolve(legacyPresent ? [legacyModel] : [])
      }
      if (command === 'delete_legacy_model_cmd') {
        legacyPresent = false
        return Promise.resolve()
      }
      return Promise.resolve()
    })

    renderSettings()

    fireEvent.click(await screen.findByRole('button', { name: 'Delete Speaker embedding (legacy)' }))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('delete_legacy_model_cmd', { filename: 'speaker_embed.onnx' }),
    )
    expect(await screen.findByText(/No legacy model files were found/i)).toBeVisible()
  })

  it('reports a cleanup catalog failure and supports retry', async () => {
    let available = false
    invoke.mockImplementation(command => {
      if (command !== 'legacy_model_cleanup_catalog_cmd') return Promise.resolve()
      return available ? Promise.resolve([]) : Promise.reject(new Error('catalog unavailable'))
    })

    renderSettings()

    expect(await screen.findByRole('alert')).toHaveTextContent('catalog unavailable')
    available = true
    fireEvent.click(screen.getByRole('button', { name: /retry/i }))
    expect(await screen.findByText(/No legacy model files were found/i)).toBeVisible()
  })

  it('attributes FFmpeg and opens the matching release-source location', async () => {
    invoke.mockResolvedValue([])
    renderSettings()
    fireEvent.click(screen.getByRole('button', { name: 'Updates' }))

    expect(screen.getByText(/uses FFmpeg under LGPL v2\.1 or later/i)).toBeVisible()
    fireEvent.click(
      screen.getByRole('button', { name: 'Open DepoAudio releases with FFmpeg source and license evidence' }),
    )
    expect(openUrl).toHaveBeenCalledWith(DEPOAUDIO_RELEASES_URL)
  })

  it('associates numeric settings with durable labels', async () => {
    invoke.mockResolvedValue([])
    const setValue = vi.fn()
    renderSettings({
      ffmpegTimeout: 300,
      setFfmpegTimeout: setValue,
      maxScanDepth: 5,
      setMaxScanDepth: setValue,
      maxFileSizeGb: 2,
      setMaxFileSizeGb: setValue,
      defaultOutputFormat: '',
      setDefaultOutputFormat: setValue,
      defaultOutputMode: '',
      setDefaultOutputMode: setValue,
    })

    fireEvent.click(screen.getByRole('button', { name: 'App' }))
    expect(screen.getByRole('spinbutton', { name: 'Processing timeout (seconds)' })).toHaveAttribute('id')
    expect(screen.getByRole('spinbutton', { name: 'Folder scan depth (levels)' })).toHaveAttribute('id')
    expect(screen.getByRole('spinbutton', { name: 'Max file size (GB)' })).toHaveAttribute('id')
  })

  it('offers a Your data section that lists local storage and needs no account', async () => {
    invoke.mockImplementation(command =>
      Promise.resolve(
        command === 'local_data_info_cmd'
          ? { appDataDir: '/synthetic/app-data', libraryReadable: true, items: [] }
          : [],
      ),
    )

    renderSettings()
    fireEvent.click(screen.getByRole('button', { name: 'Your data' }))

    expect(await screen.findByText('/synthetic/app-data')).toBeVisible()
    expect(screen.getByRole('button', { name: /export library and transcripts/i })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Clear all DepoAudio data…' })).toBeVisible()
  })

  it('passes the conversion state to Your data so Clear all waits for conversions', async () => {
    invoke.mockImplementation(command =>
      Promise.resolve(
        command === 'local_data_info_cmd'
          ? { appDataDir: '/synthetic/app-data', libraryReadable: true, items: [] }
          : [],
      ),
    )

    render(<SettingsPanel open onOpenChange={vi.fn()} prefs={{}} conversionActive />)
    fireEvent.click(screen.getByRole('button', { name: 'Your data' }))

    expect(await screen.findByText('/synthetic/app-data')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Clear all DepoAudio data…' })).toBeDisabled()
  })

  it('reloads the app instead of just closing Settings after an import', async () => {
    invoke.mockImplementation(command => {
      if (command === 'local_data_info_cmd') {
        return Promise.resolve({ appDataDir: '/synthetic/app-data', libraryReadable: true, items: [] })
      }
      if (command === 'local_data_import_cmd') {
        return Promise.resolve({
          casesAdded: 1,
          sessionsAdded: 1,
          sessionsSkipped: 0,
          filesUnavailable: 0,
          transcripts: {},
          bookmarks: [],
        })
      }
      return Promise.resolve([])
    })
    openDialog.mockResolvedValue('/synthetic/backup.zip')
    const onOpenChange = vi.fn()
    const reloadApp = vi.fn()
    render(<SettingsPanel open onOpenChange={onOpenChange} prefs={{}} reloadApp={reloadApp} />)

    fireEvent.click(screen.getByRole('button', { name: 'Your data' }))
    fireEvent.click(await screen.findByRole('button', { name: /import a backup/i }))
    expect(await screen.findByRole('status', { name: 'Import result' })).toBeVisible()

    fireEvent.keyDown(document.activeElement || document.body, { key: 'Escape' })
    await waitFor(() => expect(reloadApp).toHaveBeenCalled())
    expect(onOpenChange).not.toHaveBeenCalledWith(false)
  })

  // jsdom cannot measure layout (every height is 0), so this test checks the
  // computed-style contract that keeps cards whole, using the app's real
  // compiled stylesheet: the content pane is the only scroll container, it may
  // shrink inside the fixed-height body (min-height 0), and none of its direct
  // children can be shrunk, height-capped or given a fixed height. With those
  // in place a card's overflow-hidden clips only its own rounded corners and a
  // tall section scrolls in the pane instead of being cut off. The browser
  // check at 1160x820 and 720x600 is recorded in the CHANGELOG.
  describe('with the app stylesheet', () => {
    let removeStylesheet
    beforeAll(async () => {
      removeStylesheet = injectStylesheet(await buildAppStylesheet())
    }, 30000)
    afterAll(() => removeStylesheet?.())

    it('applies the stylesheet: an unprotected card in a flex column would shrink', () => {
      const { container } = render(
        <div className="flex flex-col">
          <div className="overflow-hidden rounded-xl">card</div>
        </div>,
      )
      const card = container.firstChild.firstChild
      expect(getComputedStyle(container.firstChild).display).toBe('flex')
      expect(getComputedStyle(card).overflow).toBe('hidden')
      expect(getComputedStyle(card).flexShrink).toBe('1')
    })

    it('scrolls only the content pane and never shrinks, caps or clips a section card', async () => {
      invoke.mockImplementation(command =>
        Promise.resolve(
          command === 'local_data_info_cmd'
            ? { appDataDir: '/synthetic/app-data', libraryReadable: true, items: [] }
            : [],
        ),
      )
      renderSettings({})

      const nav = screen.getByRole('navigation', { name: 'Settings sections' })
      const sections = [...nav.querySelectorAll('button')].map(button => button.getAttribute('aria-label'))
      expect(sections.length).toBeGreaterThan(1)
      for (const name of sections) {
        fireEvent.click(screen.getByRole('button', { name }))
        const pane = screen.getByTestId('settings-content')
        const dialog = pane.closest('[role="dialog"]')
        const paneStyle = getComputedStyle(pane)
        expect(paneStyle.overflowY).toBe('auto')
        expect(paneStyle.minHeight).toBe('0px')
        expect(paneStyle.display).toBe('flex')
        expect(paneStyle.flexDirection).toBe('column')

        // Nothing between the pane and the dialog scrolls, and the dialog
        // itself does not: the header and section list stay in place.
        expect(getComputedStyle(dialog).overflow).toBe('hidden')
        for (let el = pane.parentElement; el && el !== dialog; el = el.parentElement) {
          expect(['auto', 'scroll']).not.toContain(getComputedStyle(el).overflowY)
        }

        expect(pane.children.length).toBeGreaterThan(0)
        for (const card of pane.children) {
          const style = getComputedStyle(card)
          expect(style.flexShrink, `${name}: a section card can shrink`).toBe('0')
          expect(['', 'none'], `${name}: a section card has a max-height`).toContain(style.maxHeight)
          expect(['', 'auto'], `${name}: a section card has a fixed height`).toContain(style.height)
        }
        pane.scrollTop = 200
      }
      // Switching sections starts the new one at its top
      fireEvent.click(screen.getByRole('button', { name: sections[0] }))
      expect(screen.getByTestId('settings-content').scrollTop).toBe(0)
    })
  })
})
