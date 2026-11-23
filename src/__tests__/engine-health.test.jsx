import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'

import App from '../App'
import SettingsPanel from '../components/SettingsPanel'
import { engineProblem } from '../lib/engineHealth'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn(path => path),
  invoke: vi.fn(),
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }))
vi.mock('../hooks/PreferencesContext', () => ({
  usePreferencesContext: () => ({
    labels: ['Reporter'],
    maxScanDepth: 5,
    mode: 'stereo',
    outDir: '',
    prefsError: '',
    prefsReady: true,
    setThemePref: vi.fn(),
    themePref: 'system',
  }),
}))
vi.mock('../hooks/useTheme', () => ({
  default: () => ({ cycleTheme: vi.fn(), setThemeDirect: vi.fn(), themeLabel: 'system', themePref: 'system' }),
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
  default: () => ({ dismissed: false, error: null, progress: 0, status: 'idle', update: null }),
}))
vi.mock('../components/Convert/ConvertTab', () => ({ default: () => <section>Convert workspace</section> }))
vi.mock('../components/Player/PlayerTab', () => ({ default: () => <section>Player workspace</section> }))
vi.mock('../components/Library/LibraryTab', () => ({ default: () => <section>Library workspace</section> }))
vi.mock('../components/UpdateBanner', () => ({ default: () => null }))

const HEALTHY = {
  ffmpeg: true,
  ffprobe: true,
  ftrDecoder: true,
  ffmpegVersion: 'n7.1.1',
  os: 'windows',
  arch: 'x86_64',
}

function stubBackend(health) {
  invoke.mockImplementation(command => {
    if (command === 'health_check') return typeof health === 'function' ? health() : Promise.resolve(health)
    return Promise.resolve([])
  })
}

const banner = () => screen.queryByTestId('engine-health-banner')

describe('audio engine status in the window', () => {
  beforeEach(() => invoke.mockReset())
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('shows no engine or update status in the sidebar while the engine is healthy', async () => {
    stubBackend(HEALTHY)
    render(<App />)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('health_check'))
    await screen.findByText('Convert workspace')

    const sidebar = document.querySelector('.app-sidebar')
    expect(within(sidebar).queryByText(/FFmpeg/i)).not.toBeInTheDocument()
    expect(within(sidebar).queryByText(/Up to date|Checking engine|update/i)).not.toBeInTheDocument()
    expect(within(sidebar).queryByRole('status')).not.toBeInTheDocument()
    expect(banner()).not.toBeInTheDocument()
    // The credit stays
    expect(screen.getByRole('button', { name: 'Powered by DepoStack' })).toBeVisible()
  })

  it('warns on Convert and Player when FFmpeg is missing, and keeps it out of the sidebar', async () => {
    stubBackend({ ...HEALTHY, ffmpeg: false, ftrDecoder: false, ffmpegVersion: null })
    render(<App />)

    const alert = await screen.findByTestId('engine-health-banner')
    expect(within(alert).getByTestId('engine-health-message')).toHaveAttribute('role', 'alert')
    expect(alert).toHaveTextContent('FFmpeg is missing or did not start.')
    expect(alert).toHaveTextContent(/Reinstall DepoAudio/)
    expect(within(document.querySelector('.app-sidebar')).queryByText(/FFmpeg/i)).not.toBeInTheDocument()

    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Player' }))
    fireEvent.click(screen.getByRole('tab', { name: 'Player' }))
    await screen.findByText('Player workspace')
    expect(banner()).toBeInTheDocument()

    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Library' }))
    fireEvent.click(screen.getByRole('tab', { name: 'Library' }))
    await screen.findByText('Library workspace')
    expect(banner()).not.toBeInTheDocument()

    // Back on Convert the warning is shown again, but not read out again
    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Convert' }))
    fireEvent.click(screen.getByRole('tab', { name: 'Convert' }))
    await screen.findByText('Convert workspace')
    expect(within(banner()).getByTestId('engine-health-message')).not.toHaveAttribute('role')
  })

  it('warns when the health check itself fails, with the error behind a disclosure', async () => {
    stubBackend(() => Promise.reject(new Error('sidecar crashed')))
    render(<App />)

    const alert = await screen.findByTestId('engine-health-banner')
    expect(alert).toHaveTextContent('The audio engine could not be checked.')
    const details = within(alert).getByText('Technical details').closest('details')
    expect(within(details).getByText(/sidecar crashed/)).not.toBeVisible()
  })

  it('names the FTR decoder when only that check fails', async () => {
    stubBackend({ ...HEALTHY, ftrDecoder: false })
    render(<App />)
    const alert = await screen.findByTestId('engine-health-banner')
    expect(alert).toHaveTextContent('The FTR decoder check failed.')
    expect(alert).toHaveTextContent(/other formats should not be affected/)
  })

  it('can be dismissed, comes back on a failed re-check, and clears once fixed', async () => {
    let health = { ...HEALTHY, ffmpeg: false, ftrDecoder: false }
    stubBackend(() => Promise.resolve(health))
    render(<App />)

    await screen.findByTestId('engine-health-banner')
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss audio engine warning for this session' }))
    expect(banner()).not.toBeInTheDocument()

    // Settings > About re-check still failing: the warning returns
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }))
    fireEvent.click(await screen.findByRole('button', { name: 'About' }))
    fireEvent.click(screen.getByRole('button', { name: 'Re-check' }))
    await waitFor(() => expect(invoke.mock.calls.filter(([c]) => c === 'health_check')).toHaveLength(2))
    fireEvent.keyDown(document.activeElement, { key: 'Escape' })
    await waitFor(() => expect(banner()).toBeInTheDocument())
    // A warning that comes back after dismissal is announced again
    expect(within(banner()).getByTestId('engine-health-message')).toHaveAttribute('role', 'alert')

    health = HEALTHY
    const recheck = within(banner()).getByRole('button', { name: 'Re-check' })
    recheck.focus()
    fireEvent.click(recheck)
    await waitFor(() => expect(banner()).not.toBeInTheDocument())
    // Focus is not dropped to <body> when the banner closes
    await waitFor(() => expect(document.activeElement).toBe(document.querySelector('main')))
  })

  it('opens Settings on About from the warning', async () => {
    stubBackend({ ...HEALTHY, ftrDecoder: false })
    render(<App />)
    fireEvent.click(
      within(await screen.findByTestId('engine-health-banner')).getByRole('button', { name: /Engine details/ }),
    )
    const checks = await screen.findByTestId('engine-checks')
    expect(checks).toHaveTextContent('FTR decoderCheck failed')
    expect(screen.getByRole('button', { name: 'About' })).toHaveAttribute('aria-pressed', 'true')
  })
})

describe('Settings > About engine details', () => {
  afterEach(cleanup)

  it('lists FFmpeg version, FFprobe, the FTR decoder check and the update state', () => {
    render(
      <SettingsPanel
        open
        onOpenChange={vi.fn()}
        prefs={{}}
        health={HEALTHY}
        updater={{ status: 'idle', error: 'no updater backend' }}
        onRecheckHealth={vi.fn()}
        initialSection="about"
      />,
    )
    const checks = screen.getByTestId('engine-checks')
    expect(checks).toHaveTextContent('FFmpegn7.1.1')
    expect(checks).toHaveTextContent('FFprobeReady')
    expect(checks).toHaveTextContent('FTR decoderAvailable')
    expect(screen.getByTestId('about-update-status')).toHaveTextContent(/update check was unavailable/i)
    expect(screen.getByRole('button', { name: 'Copy diagnostics' })).toBeVisible()
  })

  it('shows a failed health check and offers a re-check', () => {
    const recheck = vi.fn()
    render(
      <SettingsPanel
        open
        onOpenChange={vi.fn()}
        prefs={{}}
        health={null}
        healthError="Audio engine health could not be checked: boom"
        onRecheckHealth={recheck}
        initialSection="about"
      />,
    )
    expect(screen.getByTestId('engine-checks')).toHaveTextContent('FFmpegNot checked')
    expect(screen.getByRole('alert')).toHaveTextContent('boom')
    fireEvent.click(screen.getByRole('button', { name: 'Re-check' }))
    expect(recheck).toHaveBeenCalled()
  })
})

describe('engineProblem', () => {
  it('is quiet when healthy or not yet checked', () => {
    expect(engineProblem(HEALTHY, '')).toBeNull()
    expect(engineProblem(null, '')).toBeNull()
  })

  it('reports the most serious problem first', () => {
    expect(engineProblem({ ffmpeg: false, ffprobe: false, ftrDecoder: false }, '').id).toBe('ffmpeg-missing')
    expect(engineProblem({ ...HEALTHY, ffprobe: false, ftrDecoder: false }, '').id).toBe('ffprobe-missing')
    expect(engineProblem(HEALTHY, 'failed').id).toBe('check-failed')
  })
})
