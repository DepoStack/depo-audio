import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import SettingsPanel from '../components/SettingsPanel'
import { DEPOAUDIO_RELEASES_URL } from '../constants'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))

function renderSettings() {
  return render(<SettingsPanel open onOpenChange={vi.fn()} prefs={{}} />)
}

describe('SettingsPanel hardware facts', () => {
  afterEach(cleanup)

  beforeEach(() => {
    invoke.mockReset()
    openUrl.mockReset()
  })

  it('does not invent core or RAM values when detection is unavailable', async () => {
    invoke.mockImplementation(command => {
      if (command === 'model_catalog_cmd') return Promise.resolve([])
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({
          acceleratorDesc: 'CoreML for eligible models; CPU fallback',
          tier: 'low',
          cpuCores: null,
          ramMb: null,
        })
      }
      return Promise.resolve()
    })

    renderSettings()

    await waitFor(() => expect(screen.getByText('CoreML for eligible models; CPU fallback')).toBeVisible())
    expect(screen.queryByText(/cores$/)).not.toBeInTheDocument()
    expect(screen.queryByText(/GB RAM$/)).not.toBeInTheDocument()
  })

  it('renders hardware facts when they were measured', async () => {
    invoke.mockImplementation(command => {
      if (command === 'model_catalog_cmd') return Promise.resolve([])
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({
          acceleratorDesc: 'CPU',
          tier: 'high',
          cpuCores: 12,
          ramMb: 32768,
        })
      }
      return Promise.resolve()
    })

    renderSettings()

    await waitFor(() => expect(screen.getByText('12 cores')).toBeVisible())
    expect(screen.getByText('32 GB RAM')).toBeVisible()
  })

  it('attributes FFmpeg and opens the matching release-source location', async () => {
    invoke.mockImplementation(command => {
      if (command === 'model_catalog_cmd') return Promise.resolve([])
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({ acceleratorDesc: 'CPU', tier: 'low', cpuCores: null, ramMb: null })
      }
      return Promise.resolve()
    })

    renderSettings()
    fireEvent.click(screen.getByRole('button', { name: 'Updates' }))

    expect(screen.getByText(/uses FFmpeg under LGPL v2\.1 or later/i)).toBeVisible()
    fireEvent.click(
      screen.getByRole('button', { name: 'Open DepoAudio releases with FFmpeg source and license evidence' }),
    )
    expect(openUrl).toHaveBeenCalledWith(DEPOAUDIO_RELEASES_URL)
  })

  it('surfaces a failed release-evidence opener action', async () => {
    invoke.mockImplementation(command => {
      if (command === 'model_catalog_cmd') return Promise.resolve([])
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({ acceleratorDesc: 'CPU', tier: 'low', cpuCores: null, ramMb: null })
      }
      return Promise.resolve()
    })
    openUrl.mockRejectedValueOnce(new Error('opener denied'))

    renderSettings()
    fireEvent.click(screen.getByRole('button', { name: 'Updates' }))
    fireEvent.click(
      screen.getByRole('button', { name: 'Open DepoAudio releases with FFmpeg source and license evidence' }),
    )

    expect(await screen.findByRole('alert')).toHaveTextContent(
      "Couldn't open DepoAudio release evidence: Error: opener denied",
    )
  })

  it('shows a model load error and retries instead of presenting a false zero count', async () => {
    let modelServicesAvailable = false
    invoke.mockImplementation(command => {
      if (!modelServicesAvailable) return Promise.reject(new Error('catalog unavailable'))
      if (command === 'model_catalog_cmd') return Promise.resolve([])
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({ acceleratorDesc: 'CPU', tier: 'low', cpuCores: null, ramMb: null })
      }
      return Promise.resolve()
    })

    renderSettings()

    expect(await screen.findByRole('alert')).toHaveTextContent('catalog unavailable')
    expect(screen.queryByText(/0\/0 active installed/i)).not.toBeInTheDocument()

    modelServicesAvailable = true
    fireEvent.click(screen.getByRole('button', { name: /retry/i }))
    await waitFor(() => expect(screen.getByText('CPU')).toBeVisible())
  })

  it('fails closed after a post-mutation catalog refresh error', async () => {
    let deleted = false
    const removableModel = {
      filename: 'optional.onnx',
      displayName: 'Optional model',
      description: 'Optional processing',
      sizeMb: 1,
      feature: 'Optional Processing',
      required: false,
      installed: true,
      removable: true,
      recommended: false,
      downloadUrl: '',
    }
    invoke.mockImplementation(command => {
      if (command === 'model_catalog_cmd') {
        return deleted ? Promise.reject(new Error('refresh unavailable')) : Promise.resolve([removableModel])
      }
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({ acceleratorDesc: 'CPU', tier: 'low', cpuCores: null, ramMb: null })
      }
      if (command === 'delete_model_cmd') {
        deleted = true
        return Promise.resolve()
      }
      return Promise.resolve()
    })

    renderSettings()
    const deleteButton = await screen.findByRole('button', { name: 'Delete Optional model' })
    fireEvent.click(deleteButton)

    expect(await screen.findByRole('alert')).toHaveTextContent('refresh unavailable')
    expect(screen.queryByRole('button', { name: 'Delete Optional model' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: /retry/i })).toBeEnabled()
  })

  it('associates numeric settings with durable labels and names collapsed navigation buttons', async () => {
    invoke.mockImplementation(command => {
      if (command === 'model_catalog_cmd') return Promise.resolve([])
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({ acceleratorDesc: 'CPU', tier: 'low', cpuCores: null, ramMb: null })
      }
      return Promise.resolve()
    })
    const setValue = vi.fn()
    render(
      <SettingsPanel
        open
        onOpenChange={vi.fn()}
        prefs={{
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
        }}
      />,
    )

    const appSection = screen.getByRole('button', { name: 'App' })
    expect(appSection).toHaveAttribute('aria-label', 'App')
    fireEvent.click(appSection)

    expect(screen.getByRole('spinbutton', { name: 'Processing timeout (seconds)' })).toHaveAttribute('id')
    expect(screen.getByRole('spinbutton', { name: 'Folder scan depth (levels)' })).toHaveAttribute('id')
    expect(screen.getByRole('spinbutton', { name: 'Max file size (GB)' })).toHaveAttribute('id')
  })

  it('separates legacy cleanup files from the active model count and removes them', async () => {
    let legacyPresent = true
    const activeModels = [
      {
        filename: 'silero_vad.onnx',
        displayName: 'Silero VAD',
        description: 'Speech detection',
        sizeMb: 2.1,
        feature: 'Speech Detection',
        required: true,
        installed: true,
        removable: false,
        recommended: true,
        downloadUrl: '',
      },
      {
        filename: 'dnsmos_sig_bak_ovr.onnx',
        displayName: 'DNSMOS',
        description: 'Quality scoring',
        sizeMb: 1.1,
        feature: 'Quality Scoring',
        required: false,
        installed: false,
        removable: false,
        recommended: true,
        downloadUrl: 'https://example.invalid/dnsmos',
      },
    ]
    const legacyModel = {
      filename: 'speaker_embed.onnx',
      displayName: 'Speaker embedding (legacy)',
      description: 'Legacy unused file — not used by this release',
      sizeMb: 4.2,
      feature: 'Legacy unused file',
      required: false,
      installed: true,
      removable: true,
      recommended: false,
      downloadUrl: '',
    }
    invoke.mockImplementation(command => {
      if (command === 'model_catalog_cmd') {
        return Promise.resolve(legacyPresent ? [...activeModels, legacyModel] : activeModels)
      }
      if (command === 'delete_model_cmd') {
        legacyPresent = false
        return Promise.resolve()
      }
      if (command === 'system_capabilities_cmd') {
        return Promise.resolve({ acceleratorDesc: 'CPU', tier: 'low', cpuCores: null, ramMb: null })
      }
      return Promise.resolve()
    })

    renderSettings()

    await waitFor(() => expect(screen.getByText('1/2 active installed · 2.1 MB')).toBeVisible())
    expect(screen.getByRole('status')).toHaveTextContent(
      '1 legacy unused file (4.2 MB) can be removed below. These files are not used by this release.',
    )
    expect(screen.queryByText('2/3 installed')).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Delete Speaker embedding (legacy)' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('delete_model_cmd', { filename: 'speaker_embed.onnx' }))
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument())
  })
})
