import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import FileRow from '../components/Convert/FileRow'
import { isPlayerPlayable } from '../lib/player'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  convertFileSrc: path => `asset://${path}`,
}))

vi.mock('../components/common/Waveform', () => ({
  default: ({ audioPath }) => <div data-testid="waveform" data-path={audioPath} />,
}))

const file = { path: '/case/hearing.trm', name: 'hearing.trm', fmt: null }

function doneJob(outputs) {
  return { status: 'done', outputs }
}

describe('Completed conversion rows', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue()
  })
  afterEach(cleanup)

  it('offers Reveal and Open in Player for a single output', async () => {
    const onOpenInPlayer = vi.fn()
    render(
      <FileRow
        file={file}
        job={doneJob([{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1024 }])}
        onRemove={vi.fn()}
        converting={false}
        onOpenInPlayer={onOpenInPlayer}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Reveal in folder: hearing.mp3' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: '/out/hearing.mp3' }))

    fireEvent.click(screen.getByRole('button', { name: 'Open in Player: hearing.mp3' }))
    expect(onOpenInPlayer).toHaveBeenCalledWith(['/out/hearing.mp3'])
  })

  it('offers the same actions for split outputs', async () => {
    const onOpenInPlayer = vi.fn()
    const outputs = [
      { path: '/out/hearing_ch1.wav', name: 'hearing_ch1.wav', size: 1 },
      { path: '/out/hearing_ch2.wav', name: 'hearing_ch2.wav', size: 1 },
    ]
    render(
      <FileRow
        file={file}
        job={doneJob(outputs)}
        onRemove={vi.fn()}
        converting={false}
        onOpenInPlayer={onOpenInPlayer}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Reveal in folder: the 2 outputs of hearing.trm' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('show_in_folder', { path: '/out/hearing_ch1.wav' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open in Player: the 2 outputs of hearing.trm' }))
    expect(onOpenInPlayer).toHaveBeenCalledWith(outputs.map(out => out.path))
  })

  it('shows a reveal failure instead of hiding it', async () => {
    invoke.mockRejectedValue(new Error('opener unavailable'))
    render(
      <FileRow
        file={file}
        job={doneJob([{ path: '/out/hearing.flac', name: 'hearing.flac', size: 1 }])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Reveal in folder: hearing.flac' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('opener unavailable')
    // Without a Player handler there is no Open in Player button
    expect(screen.queryByRole('button', { name: /Open in Player/ })).not.toBeInTheDocument()
  })

  it('gives each action an accessible name that starts with its visible label (WCAG 2.5.3)', () => {
    for (const outputs of [
      [{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1 }],
      [
        { path: '/out/hearing_ch1.wav', name: 'hearing_ch1.wav', size: 1 },
        { path: '/out/hearing_ch2.wav', name: 'hearing_ch2.wav', size: 1 },
      ],
    ]) {
      const { unmount } = render(
        <FileRow file={file} job={doneJob(outputs)} onRemove={vi.fn()} converting={false} onOpenInPlayer={vi.fn()} />,
      )
      for (const visible of ['Reveal in folder', 'Open in Player']) {
        const button = screen.getByText(visible).closest('button')
        expect(button.textContent.trim()).toBe(visible)
        expect(button.getAttribute('aria-label').startsWith(visible)).toBe(true)
      }
      unmount()
    }
  })

  it('only sends formats the Player can open as they are', () => {
    for (const ext of ['wav', 'mp3', 'flac', 'opus', 'm4a']) expect(isPlayerPlayable(`/out/a.${ext}`)).toBe(true)
    for (const ext of ['trm', 'sgmca', 'ftr', 'txt']) expect(isPlayerPlayable(`/out/a.${ext}`)).toBe(false)
  })
})

describe('Queued rows with several audio tracks', () => {
  afterEach(cleanup)

  const streams = {
    convertedIndex: 0,
    streams: [
      { channels: 1, sampleRate: 48000, language: 'eng', title: 'Floor' },
      { channels: 4, sampleRate: 48000, language: 'spa', title: 'Interpreter' },
    ],
  }

  it('says which track is converted', () => {
    const recording = { path: '/case/hearing.mkv', name: 'hearing.mkv', fmt: null }
    render(<FileRow file={recording} streams={streams} onRemove={vi.fn()} converting={false} />)
    expect(
      screen.getByText(
        'This file has 2 audio tracks; converting track 1 (eng, "Floor"). The other track is not converted.',
      ),
    ).toBeTruthy()
  })

  it('shows no notice for a single track', () => {
    const recording = { path: '/case/hearing.wav', name: 'hearing.wav', fmt: null }
    render(
      <FileRow
        file={recording}
        streams={{ convertedIndex: 0, streams: [streams.streams[0]] }}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    expect(screen.queryByText(/audio tracks/)).toBeNull()
  })
})

describe('Queued FTR recordings', () => {
  afterEach(cleanup)

  const trmStreams = openEnded => ({
    convertedIndex: 0,
    streams: [{ channels: 4, sampleRate: 16000, language: null, title: null }],
    durationSecs: 12.8,
    trm: { openEnded, sessionFirst: true, sessionLast: false, decodePath: 'deinterleaved' },
  })
  const recording = { path: '/case/CR1_20260101-0900_01d409706d824cb0.trm', name: 'CR1.trm', fmt: null }

  it('says when the recorder did not close the file', () => {
    render(<FileRow file={recording} streams={trmStreams(true)} onRemove={vi.fn()} converting={false} />)
    expect(
      screen.getByText(
        "This recording's header says it was never closed (or is still recording); DepoAudio read the audio written so far.",
      ),
    ).toBeTruthy()
  })

  it('shows the session notice it is given and nothing for a closed file', () => {
    render(
      <FileRow
        file={recording}
        streams={trmStreams(false)}
        sessionNotice="No queued file is marked as this session's last file; if the session has later files, add them too."
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    expect(screen.queryByText(/not closed properly/)).toBeNull()
    expect(
      screen.getByText(
        "No queued file is marked as this session's last file; if the session has later files, add them too.",
      ),
    ).toBeTruthy()
  })
})

describe('Compare with original', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue([])
  })
  afterEach(cleanup)

  const wavSource = { path: '/case/hearing.wav', name: 'hearing.wav', fmt: null }
  // The compare player's own media elements (the row's mini players come first)
  const compareAudio = () => [...document.querySelectorAll('audio')].slice(-2)
  const withClock = audio => {
    let time = 0
    Object.defineProperty(audio, 'currentTime', {
      configurable: true,
      get: () => time,
      set: value => {
        time = value
      },
    })
    return audio
  }

  it('opens a before/after player for a source the web view can play', () => {
    render(
      <FileRow
        file={wavSource}
        job={doneJob([{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1 }])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    const button = screen.getByRole('button', { name: 'Compare with original: hearing.mp3' })
    expect(button).toBeEnabled()
    expect(button).toHaveAttribute('aria-expanded', 'false')
    fireEvent.click(button)
    expect(button).toHaveAttribute('aria-expanded', 'true')

    const original = screen.getByRole('button', { name: 'Original' })
    const output = screen.getByRole('button', { name: 'Output' })
    expect(output).toHaveAttribute('aria-pressed', 'true')
    const sources = compareAudio().map(audio => audio.getAttribute('src'))
    expect(sources).toEqual(['asset:///case/hearing.wav', 'asset:///out/hearing.mp3'])

    fireEvent.click(original)
    expect(original).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByText(/Listening to the original/)).toBeInTheDocument()

    fireEvent.click(button)
    expect(screen.queryByRole('button', { name: 'Original' })).not.toBeInTheDocument()
  })

  it('keeps the position when switching between original and output', () => {
    render(
      <FileRow
        file={wavSource}
        job={doneJob([{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1 }])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: /Compare with original/ }))
    const [originalAudio, outputAudio] = compareAudio().map(withClock)
    outputAudio.currentTime = 12
    fireEvent.timeUpdate(outputAudio)
    fireEvent.click(screen.getByRole('button', { name: 'Original' }))
    expect(originalAudio.currentTime).toBe(12)
  })

  it('is disabled with a plain reason when the original is a court container', () => {
    render(
      <FileRow
        file={file}
        job={doneJob([{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1 }])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    const button = screen.getByRole('button', { name: 'Compare with original: hearing.mp3' })
    // Focusable, so keyboard users reach the reason, but marked unavailable.
    expect(button).toHaveAttribute('aria-disabled', 'true')
    expect(button).not.toHaveAttribute('disabled')
    expect(button).toHaveClass('line-through')
    expect(button).toHaveAccessibleDescription(
      "(unavailable: Original can't be played here; open the output in Player.)",
    )
    expect(screen.getByText("(unavailable: Original can't be played here; open the output in Player.)")).toBeVisible()
    button.focus()
    expect(button).toHaveFocus()
    fireEvent.click(button)
    expect(screen.queryByRole('button', { name: 'Original' })).not.toBeInTheDocument()
    expect(button).not.toHaveAttribute('aria-expanded')
  })

  it('says positions do not line up after leading-silence trim', () => {
    render(
      <FileRow
        file={wavSource}
        job={{ ...doneJob([{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1 }]), trimmed: true }}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: /Compare with original/ }))
    expect(screen.getByText(/Trim silence removed audio from the start of the output/)).toBeVisible()
    expect(screen.getByText(/Listening to the output/)).toHaveTextContent('choose the other button to switch')
    expect(screen.queryByText(/at the same position/)).not.toBeInTheDocument()
  })

  it('gives a next step when the output fails to load', () => {
    render(
      <FileRow
        file={wavSource}
        job={doneJob([{ path: '/out/hearing.opus', name: 'hearing.opus', size: 1 }])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: /Compare with original/ }))
    const [, outputAudio] = compareAudio()
    fireEvent.error(outputAudio)
    expect(screen.getByRole('alert')).toHaveTextContent(
      "Output can't be played here. Use Reveal in folder to open it in another player.",
    )
  })

  it('compares the first output of a split conversion and says which', () => {
    render(
      <FileRow
        file={wavSource}
        job={doneJob([
          { path: '/out/hearing_ch1.wav', name: 'hearing_ch1.wav', size: 1 },
          { path: '/out/hearing_ch2.wav', name: 'hearing_ch2.wav', size: 1 },
        ])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Compare with original: hearing_ch1.wav' }))
    expect(screen.getByText(/Comparing hearing_ch1\.wav with the original/)).toBeInTheDocument()
    const sources = compareAudio().map(audio => audio.getAttribute('src'))
    expect(sources).toEqual(['asset:///case/hearing.wav', 'asset:///out/hearing_ch1.wav'])
  })

  it('says so when the original fails to load at runtime', () => {
    render(
      <FileRow
        file={wavSource}
        job={doneJob([{ path: '/out/hearing.mp3', name: 'hearing.mp3', size: 1 }])}
        onRemove={vi.fn()}
        converting={false}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: /Compare with original/ }))
    const [originalAudio] = compareAudio()
    fireEvent.error(originalAudio)
    fireEvent.click(screen.getByRole('button', { name: 'Original' }))
    expect(screen.getByRole('alert')).toHaveTextContent("Original can't be played here; open the output in Player.")
  })
})
