import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import ConvertTab from '../components/Convert/ConvertTab'

const preferencesContext = vi.hoisted(() => ({ current: null }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
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
    const catalyst = screen.getByRole('button', { name: /For CATalyst audio sync \(WAV\)/ })
    expect(catalyst).toHaveAccessibleDescription(/Stenograph lists WAV for audio association/)
    expect(picker.textContent.match(/Not tested in CAT software/g)).toHaveLength(7)
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
      'Custom: the earlier “Quick Share” preset. Closest new preset: Smallest file (Opus) — CATalyst 20 or newer',
    )
    expect(screen.getByTestId('preset-picker').querySelectorAll('[aria-pressed="true"]')).toHaveLength(0)
    expect(screen.getByTestId('opus-catalyst-note')).toHaveTextContent(
      'Stenograph says Opus plays only in CATalyst 20 or newer',
    )
    for (const setter of ['setFormatOut', 'setTrim', 'setFade', 'setFadeDur', 'setMode', 'setNormalize']) {
      expect(prefs[setter]).not.toHaveBeenCalled()
    }
  })

  it('marks the matching preset as in use', () => {
    preferencesContext.current = { ...preferences(), normalize: true, hpf: true, autoLevel: true }
    render(<ConvertTab {...props()} />)

    expect(screen.getByRole('button', { name: /For CATalyst audio sync/ })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByTestId('preset-status')).toHaveTextContent('Using: For CATalyst audio sync (WAV)')
    expect(screen.queryByTestId('opus-catalyst-note')).not.toBeInTheDocument()
  })
})
