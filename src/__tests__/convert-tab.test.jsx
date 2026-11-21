import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { openUrl } from '@tauri-apps/plugin-opener'
import ConvertTab from '../components/Convert/ConvertTab'
import { STENOGRAPH_ASSOCIATE_AUDIO, STENOGRAPH_SINGLE_CHANNEL } from '../presets'
import FormatTable from '../components/Convert/FormatTable'

const preferencesContext = vi.hoisted(() => ({ current: null }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }))
vi.mock('../hooks/PreferencesContext', () => ({
  usePreferencesContext: () => preferencesContext.current,
}))

function preferences() {
  return {
    mode: 'stereo',
    setMode: vi.fn(),
    formatOut: 'wav',
    setFormatOut: vi.fn(),
    labels: [],
    setLabels: vi.fn(),
    chanVols: [],
    setChanVols: vi.fn(),
    outDir: '',
    setOutDir: vi.fn(),
    rate: '48000',
    setRate: vi.fn(),
    mp3Bitrate: 192,
    setMp3Bitrate: vi.fn(),
    normalize: false,
    setNormalize: vi.fn(),
    trim: false,
    setTrim: vi.fn(),
    fade: false,
    setFade: vi.fn(),
    fadeDur: 0.5,
    setFadeDur: vi.fn(),
    hpf: false,
    setHpf: vi.fn(),
    denoise: false,
    setDenoise: vi.fn(),
    denoiseQuality: 'fast',
    setDenoiseQuality: vi.fn(),
    autoLevel: false,
    setAutoLevel: vi.fn(),
    declip: false,
    setDeclip: vi.fn(),
    enhance: false,
    setEnhance: vi.fn(),
    dereverb: false,
    setDereverb: vi.fn(),
  }
}

const firstFile = { path: 'C:\\case\\first.trm', name: 'first.trm', fmt: null }

function props(files = [firstFile]) {
  return {
    capabilities: { dereverbAvailable: false },
    files,
    dragOver: false,
    caseName: '',
    setCaseName: vi.fn(),
    onDragOver: vi.fn(),
    onDragLeave: vi.fn(),
    onDrop: vi.fn(),
    browseFiles: vi.fn(),
    browseOutDir: vi.fn(),
    removeFile: vi.fn(),
    clearAll: vi.fn(),
    jobs: {},
    converting: false,
    startConversion: vi.fn(),
    cancelConversion: vi.fn(),
    doneCount: 0,
    failCount: 0,
  }
}

describe('ConvertTab scan cancellation', () => {
  afterEach(cleanup)

  beforeEach(() => {
    preferencesContext.current = preferences()
    invoke.mockReset()
    invoke.mockResolvedValue()
    listen.mockReset()
    listen.mockResolvedValue(vi.fn())
  })

  it('keeps a failed user cancellation visible and retryable', async () => {
    let cancelAttempts = 0
    invoke.mockImplementation(command => {
      if (command === 'analyze_audio_cmd') return new Promise(() => {})
      if (command === 'cancel_scan_cmd') {
        cancelAttempts += 1
        return cancelAttempts === 1 ? Promise.reject(new Error('cancel IPC unavailable')) : Promise.resolve()
      }
      return Promise.resolve()
    })
    render(<ConvertTab {...props()} />)

    fireEvent.click(screen.getByRole('button', { name: 'Scan' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('analyze_audio_cmd', { path: firstFile.path }))
    expect(screen.getByRole('status', { name: 'Recording scan status' })).toHaveTextContent('Scanning 1 of 1')
    expect(screen.getByRole('progressbar', { name: 'Recording scan progress' })).toHaveAttribute('aria-valuenow', '0')

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Could not stop the scan'))
    expect(screen.getByRole('alert')).toHaveTextContent('cancel IPC unavailable')
    expect(screen.getByRole('button', { name: 'Retry cancel' })).toBeEnabled()

    fireEvent.click(screen.getByRole('button', { name: 'Retry cancel' }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Scan' })).toBeEnabled())
    expect(invoke.mock.calls.filter(([command]) => command === 'cancel_scan_cmd')).toHaveLength(2)
  }, 10000)

  it('keeps Scan blocked when a queue-change generation bump fails', async () => {
    let resolveAnalysis
    let cancellationAllowed = false
    invoke.mockImplementation(command => {
      if (command === 'analyze_audio_cmd') {
        return new Promise(resolve => {
          resolveAnalysis = resolve
        })
      }
      if (command === 'cancel_scan_cmd') {
        return cancellationAllowed ? Promise.resolve() : Promise.reject(new Error('epoch bump failed'))
      }
      return Promise.resolve()
    })
    const view = render(<ConvertTab {...props()} />)

    fireEvent.click(screen.getByRole('button', { name: 'Scan' }))
    await waitFor(() => expect(resolveAnalysis).toBeTypeOf('function'))
    view.rerender(
      <ConvertTab {...props([{ path: 'C:\\case\\replacement.trm', name: 'replacement.trm', fmt: null }])} />,
    )
    resolveAnalysis({ recommendations: [] })

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('active scan could not be stopped'))
    expect(screen.getByRole('alert')).toHaveTextContent('epoch bump failed')
    expect(screen.queryByRole('button', { name: 'Scan' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry cancel' })).toBeEnabled()

    cancellationAllowed = true
    fireEvent.click(screen.getByRole('button', { name: 'Retry cancel' }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Scan' })).toBeEnabled())
  })

  it('does not advertise learned-model hardware tiers or acceleration', () => {
    render(
      <ConvertTab
        {...props()}
        capabilities={{
          dereverbAvailable: false,
          tier: 'low',
          cpuCores: null,
          ramMb: null,
          appleSilicon: false,
        }}
      />,
    )

    expect(screen.queryByText(/Lightweight mode|High performance|Apple Silicon|GB RAM|cores/)).not.toBeInTheDocument()
  })

  it('marks the guided workflow for compact three-column reflow', () => {
    render(<ConvertTab {...props([])} />)

    const progress = screen.getByRole('list', { name: 'Conversion progress' })
    expect(progress).toHaveClass('conversion-stepper')
    expect(progress.children).toHaveLength(3)
    expect([...progress.children].every(step => step.classList.contains('conversion-step'))).toBe(true)
  })

  it('leads with the court-audio task, local-processing boundary, and a native browse action', () => {
    const emptyQueue = props([])
    render(<ConvertTab {...emptyQueue} />)

    expect(screen.getByRole('heading', { level: 2, name: 'Convert court recordings' })).toBeVisible()
    expect(screen.getByText('Recordings and processing stay on this computer')).toBeVisible()

    const browse = screen.getByRole('button', { name: 'Add audio files: browse for recordings' })
    fireEvent.click(browse)
    expect(emptyQueue.browseFiles).toHaveBeenCalledTimes(1)

    const convert = screen.getByRole('button', { name: 'Convert' })
    expect(convert).toBeDisabled()
    expect(convert).toHaveAttribute('title', 'Add at least one recording to continue.')
    expect(screen.getByRole('status', { name: 'Conversion status' })).toHaveTextContent(
      'Add a recording to choose an output and convert.',
    )
  })

  it('only suggests scan fixes until the person applies them', async () => {
    invoke.mockImplementation(command => {
      if (command === 'analyze_audio_cmd') {
        return Promise.resolve({
          needsLeveling: true,
          hasClipping: true,
          recommendations: ['Channel level spread 9.5 dB'],
        })
      }
      return Promise.resolve()
    })
    const prefs = preferencesContext.current
    render(<ConvertTab {...props()} />)

    fireEvent.click(screen.getByRole('button', { name: 'Scan' }))
    const apply = await screen.findByRole('button', { name: 'Apply suggestions' })
    expect(screen.getByText(/Nothing is switched on until you apply them/)).toBeVisible()
    expect(
      screen.getByText('Scan checks the first 3 minutes of each file. Anything after that is not measured.'),
    ).toBeVisible()
    expect(screen.getAllByText(/^Recommended —/)).toHaveLength(2)
    expect(prefs.setAutoLevel).not.toHaveBeenCalled()
    expect(prefs.setDeclip).not.toHaveBeenCalled()
    expect(screen.getByRole('switch', { name: /Balance Microphone Levels/ })).not.toBeChecked()

    // A single suggestion can still be turned on by hand
    fireEvent.click(screen.getByRole('switch', { name: /Fix Clipped Audio/ }))
    expect(prefs.setDeclip).toHaveBeenCalledWith(true)
    prefs.setDeclip.mockClear()

    fireEvent.click(apply)
    expect(prefs.setAutoLevel).toHaveBeenCalledWith(true)
    expect(prefs.setDeclip).toHaveBeenCalledWith(true)
  })

  it('says before a scan that it checks only the first 3 minutes', () => {
    render(<ConvertTab {...props()} />)
    expect(screen.getByText(/Scan checks the first 3 minutes of each file\./)).toBeVisible()
  })
})

describe('ConvertTab WAV size estimate', () => {
  afterEach(cleanup)

  beforeEach(() => {
    preferencesContext.current = preferences()
    invoke.mockReset()
    invoke.mockResolvedValue()
    listen.mockReset()
    listen.mockResolvedValue(vi.fn())
  })

  const audioStreams = { [firstFile.path]: { convertedIndex: 0, streams: [{ channels: 2 }], durationSecs: 60 } }

  it('shows an estimate next to the WAV choice', () => {
    render(<ConvertTab {...props()} audioStreams={audioStreams} />)
    expect(screen.getByText(/^Estimated WAV size: about 11\.0 MB for this file/)).toBeInTheDocument()
  })

  it('shows no estimate for other formats', () => {
    preferencesContext.current = { ...preferences(), formatOut: 'mp3' }
    render(<ConvertTab {...props()} audioStreams={audioStreams} />)
    expect(screen.queryByText(/Estimated WAV size/)).toBeNull()
  })
})

describe('Closed-format guidance', () => {
  afterEach(cleanup)

  beforeEach(() => {
    preferencesContext.current = preferences()
    invoke.mockReset()
    invoke.mockResolvedValue()
    listen.mockReset()
    listen.mockResolvedValue(vi.fn())
  })

  it('shows each rejected file with the guidance the backend gave for it', () => {
    const rejected = [
      {
        path: 'C:\\case\\memo.ds2',
        name: 'memo.ds2',
        fmt: {
          key: 'ds2',
          name: 'DSS Pro Dictation (DS2)',
          status: 'unsupported',
          handler: 'rejected',
          note: 'Export to WAV with ODMS.',
        },
      },
      {
        path: 'C:\\case\\photo.dcr',
        name: 'photo.dcr',
        fmt: {
          key: 'dcr-image',
          name: 'Camera Raw Image (DCR)',
          status: 'unsupported',
          handler: 'rejected',
          note: 'This .dcr looks like a camera image, not court audio.',
        },
      },
    ]
    render(<ConvertTab {...props(rejected)} />)

    const alert = screen.getByRole('alert')
    expect(alert).toHaveTextContent('Remove 2 unsupported files before converting.')
    expect(alert).toHaveTextContent('memo.ds2: Export to WAV with ODMS.')
    expect(alert).toHaveTextContent('photo.dcr: This .dcr looks like a camera image, not court audio.')
    expect(alert).not.toHaveTextContent('AES/DCR')
    expect(screen.getByRole('button', { name: 'Convert 2 Files' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Convert 2 Files' })).toHaveAttribute(
      'title',
      'Remove unsupported files first. The note for each one says how to export it.',
    )
    cleanup()

    // A camera image is not a recording, so there is nothing to export.
    render(<ConvertTab {...props([rejected[1]])} />)
    const convert = screen.getByRole('button', { name: 'Convert' })
    expect(convert).toHaveAttribute('title', 'Remove the files that are not recordings first.')
    expect(screen.getByText('Remove the files that are not recordings first.')).toBeVisible()
    expect(screen.queryByText(/export them to WAV/)).not.toBeInTheDocument()
  })

  it('falls back to neutral guidance when a rejected file has no note', () => {
    render(
      <ConvertTab
        {...props([
          { path: '/case/a.aes', name: 'a.aes', fmt: { name: 'Eclipse', status: 'unsupported', note: null } },
        ])}
      />,
    )
    expect(screen.getByRole('alert')).toHaveTextContent(
      'a.aes: Export it to WAV from the software that recorded it first.',
    )
  })

  it('lists the closed formats in their own section without naming one vendor for .dcr', () => {
    render(<FormatTable />)
    fireEvent.click(screen.getByRole('button', { name: /Supported formats/ }))

    expect(screen.getByText('Closed formats — export to WAV first')).toBeVisible()
    for (const ext of ['.aes', '.dcr', '.ds2', '.arf  .wrf', '.msv  .dvf']) {
      expect(screen.getByText(ext, { normalizer: text => text })).toBeVisible()
    }
    expect(screen.getByText('Liberty Court Recorder · BIS Digital')).toBeVisible()
    expect(screen.queryByText(/High Criteria/)).not.toBeInTheDocument()
  })
})

describe('ConvertTab preset picker', () => {
  afterEach(cleanup)

  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue()
    listen.mockReset()
    listen.mockResolvedValue(vi.fn())
  })

  it('shows each preset with its basis and that it is not tested in CAT software', () => {
    preferencesContext.current = preferences()
    render(<ConvertTab {...props()} />)

    const picker = screen.getByTestId('preset-picker')
    // The name is only the preset's name; the basis and the not-tested line
    // are read once, as the description.
    const catalyst = screen.getByRole('button', { name: 'For CATalyst audio sync (WAV)' })
    expect(catalyst).toHaveAccessibleDescription(
      'Stenograph lists WAV for audio association and says to convert MP3 to WAV for CATalyst 21 or older. Not tested in CAT software',
    )
    expect(screen.getByRole('button', { name: 'Smallest file (Opus)' })).toHaveAccessibleDescription(
      /Opus plays only in CATalyst 20 or newer.*Not tested in CAT software$/,
    )
    expect(picker.textContent.match(/Not tested in CAT software/g)).toHaveLength(7)
  })

  it('links the Stenograph articles the format notes come from', async () => {
    preferencesContext.current = preferences()
    openUrl.mockClear()
    render(<ConvertTab {...props()} />)

    fireEvent.click(screen.getByRole('button', { name: /^Associate Audio File \(Stenograph Solution Center/ }))
    fireEvent.click(screen.getByRole('button', { name: /^Single Channel Audio \(Stenograph Solution Center/ }))
    expect(openUrl.mock.calls).toEqual([[STENOGRAPH_ASSOCIATE_AUDIO], [STENOGRAPH_SINGLE_CHANNEL]])

    openUrl.mockRejectedValueOnce(new Error('no browser'))
    fireEvent.click(screen.getByRole('button', { name: /^Associate Audio File/ }))
    expect(await screen.findByRole('alert')).toHaveTextContent(
      "Couldn't open Stenograph's “Associate Audio File” article: Error: no browser",
    )
  })

  it('does not call default settings a chosen preset', () => {
    preferencesContext.current = preferences()
    render(<ConvertTab {...props()} />)

    expect(screen.getByTestId('preset-status')).toHaveTextContent(
      'No preset chosen. The current settings are the same as “For other or older CAT software (WAV)”.',
    )
    expect(screen.getByTestId('preset-status')).not.toHaveTextContent('Using:')
    expect(screen.getByTestId('preset-picker').querySelectorAll('[aria-pressed="true"]')).toHaveLength(0)

    fireEvent.click(screen.getByRole('button', { name: 'For other or older CAT software (WAV)' }))
    expect(screen.getByTestId('preset-status')).toHaveTextContent('Using: For other or older CAT software (WAV)')
    expect(screen.getByRole('button', { name: 'For other or older CAT software (WAV)' })).toHaveAttribute(
      'aria-pressed',
      'true',
    )
  })

  it('applies a preset only when pressed', () => {
    const prefs = preferences()
    preferencesContext.current = prefs
    render(<ConvertTab {...props()} />)

    expect(prefs.setFormatOut).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: /Send to scopist/ }))
    expect(prefs.setFormatOut).toHaveBeenCalledWith('mp3')
    expect(prefs.setMp3Bitrate).toHaveBeenCalledWith(192)
    expect(prefs.setTrim).toHaveBeenCalledWith(false)
    expect(prefs.setFadeDur).not.toHaveBeenCalled()
  })

  it('shows Custom and the closest new preset for saved Quick Share settings, without changing them', () => {
    const prefs = {
      ...preferences(),
      formatOut: 'opus',
      normalize: true,
      trim: true,
      fade: true,
      fadeDur: 0.3,
      hpf: true,
    }
    preferencesContext.current = prefs
    render(<ConvertTab {...props()} />)

    expect(screen.getByTestId('preset-status')).toHaveTextContent(
      'Custom: the earlier “Quick Share” preset. Closest new preset: Smallest file (Opus)',
    )
    expect(screen.getByTestId('preset-picker').querySelectorAll('[aria-pressed="true"]')).toHaveLength(0)
    expect(screen.getByTestId('opus-catalyst-note')).toHaveTextContent(
      'Stenograph says Opus plays only in CATalyst 20 or newer',
    )
    for (const setter of ['setFormatOut', 'setTrim', 'setFade', 'setFadeDur', 'setMode', 'setNormalize']) {
      expect(prefs[setter]).not.toHaveBeenCalled()
    }
  })

  it('marks a chosen preset as in use while the settings still match it', () => {
    preferencesContext.current = { ...preferences(), normalize: true, hpf: true, autoLevel: true }
    const view = render(<ConvertTab {...props()} />)

    expect(screen.getByRole('button', { name: /For CATalyst audio sync/ })).toHaveAttribute('aria-pressed', 'false')
    fireEvent.click(screen.getByRole('button', { name: /For CATalyst audio sync/ }))
    expect(screen.getByRole('button', { name: /For CATalyst audio sync/ })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByTestId('preset-status')).toHaveTextContent('Using: For CATalyst audio sync (WAV)')
    expect(screen.queryByTestId('opus-catalyst-note')).not.toBeInTheDocument()

    // Changing a setting by hand ends the match, so the preset is no longer in use.
    preferencesContext.current = { ...preferencesContext.current, trim: true }
    view.rerender(<ConvertTab {...props()} />)
    expect(screen.getByTestId('preset-status')).toHaveTextContent('Custom settings')
    expect(screen.getByTestId('preset-picker').querySelectorAll('[aria-pressed="true"]')).toHaveLength(0)
  })
})
