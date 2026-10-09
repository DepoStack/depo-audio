import { useState } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import SettingsPanel from '../components/SettingsPanel'
import { DEPOAUDIO_RELEASES_URL } from '../constants'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))

function renderSettings(prefs = {}) {
  return render(<SettingsPanel open onOpenChange={vi.fn()} prefs={prefs} />)
}

describe('SettingsPanel release boundaries', () => {
  afterEach(cleanup)

  beforeEach(() => {
    invoke.mockReset()
    openUrl.mockReset()
  })

  it('opens on app settings and checks legacy storage only when requested', () => {
    invoke.mockResolvedValue([])
    renderSettings({ ffmpegTimeout: 300, maxScanDepth: 5, maxFileSizeGb: 2 })

    expect(screen.getByRole('button', { name: 'App' })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByRole('spinbutton', { name: 'Processing timeout (seconds)' })).toBeVisible()
    expect(invoke).not.toHaveBeenCalledWith('legacy_model_cleanup_catalog_cmd')
    fireEvent.click(screen.getByRole('button', { name: 'Model files' }))
    expect(invoke).toHaveBeenCalledWith('legacy_model_cleanup_catalog_cmd')
  })

  it('shows a deletion-only empty state without requesting capabilities or model downloads', async () => {
    invoke.mockResolvedValue([])

    renderSettings()
    fireEvent.click(screen.getByRole('button', { name: 'Model files' }))

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
    fireEvent.click(screen.getByRole('button', { name: 'Model files' }))

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
    fireEvent.click(screen.getByRole('button', { name: 'Model files' }))

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
})

function NumericSettings() {
  const [ffmpegTimeout, setFfmpegTimeout] = useState(300)
  const [maxScanDepth, setMaxScanDepth] = useState(5)
  const [maxFileSizeGb, setMaxFileSizeGb] = useState(2)
  return (
    <>
      <SettingsPanel
        open
        onOpenChange={() => {}}
        prefs={{ ffmpegTimeout, setFfmpegTimeout, maxScanDepth, setMaxScanDepth, maxFileSizeGb, setMaxFileSizeGb }}
      />
      <output data-testid="saved-settings">{JSON.stringify({ ffmpegTimeout, maxScanDepth, maxFileSizeGb })}</output>
    </>
  )
}

describe('numeric settings validation', () => {
  afterEach(cleanup)

  it('keeps fractional integer drafts local and accepts a subsequent valid value', () => {
    invoke.mockResolvedValue([])
    render(<NumericSettings />)
    fireEvent.click(screen.getByRole('button', { name: 'App' }))
    const depth = screen.getByRole('spinbutton', { name: 'Folder scan depth (levels)' })
    fireEvent.change(depth, { target: { value: '5.5' } })
    expect(depth).toHaveAttribute('aria-invalid', 'true')
    expect(screen.getByRole('alert')).toHaveTextContent('whole number')
    expect(screen.getByTestId('saved-settings')).toHaveTextContent('"maxScanDepth":5,')
    fireEvent.blur(depth)
    expect(screen.getByTestId('saved-settings')).not.toHaveTextContent('5.5')
    fireEvent.change(depth, { target: { value: '6' } })
    expect(depth).not.toHaveAttribute('aria-invalid', 'true')
    expect(screen.getByTestId('saved-settings')).toHaveTextContent('"maxScanDepth":6,')

    fireEvent.change(screen.getByRole('spinbutton', { name: 'Max file size (GB)' }), {
      target: { value: '2.5' },
    })
    expect(screen.getByTestId('saved-settings')).toHaveTextContent('"maxFileSizeGb":2.5')
  })
})
