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
