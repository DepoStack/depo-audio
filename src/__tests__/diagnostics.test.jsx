import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'

import packageJson from '../../package.json'
import AppErrorBoundary from '../components/AppErrorBoundary'
import SettingsPanel from '../components/SettingsPanel'
import { APP_VERSION, buildDiagnostics, errorTypeOf, rememberEngineInfo } from '../lib/diagnostics'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))

// Everything a path or a file name could leak: folder separators, a drive,
// a user folder, a case name, and recording file names.
const LEAKS = ['/', '\\', 'C:', 'Users', 'reporter', 'Smith', 'Jones', '.wav', '.trm', 'witness']
const expectNoPathOrName = text => {
  for (const leak of LEAKS) expect(text, `diagnostics must not contain ${leak}`).not.toContain(leak)
}

let clipboardText
beforeEach(() => {
  clipboardText = null
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: vi.fn(async text => (clipboardText = text)) },
  })
  invoke.mockReset()
  invoke.mockResolvedValue([])
  rememberEngineInfo(null)
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('diagnostics text', () => {
  it('uses the release version from package.json', () => {
    expect(APP_VERSION).toBe(packageJson.version)
  })

  it('lists version, OS, FFmpeg version and error type', () => {
    const text = buildDiagnostics({
      os: 'windows',
      arch: 'x86_64',
      ffmpegVersion: 'n7.1.1-20250301',
      errorType: 'TypeError',
    })
    expect(text).toBe(
      [
        'DepoAudio diagnostics',
        `App version: ${packageJson.version}`,
        'OS: windows (x86_64)',
        'FFmpeg: n7.1.1-20250301',
        'Error type: TypeError',
      ].join('\n'),
    )
    expect(buildDiagnostics({})).toContain('Error type: none')
    expect(buildDiagnostics({})).toContain('FFmpeg: not available')
  })

  it('never carries a file path or name, whatever it is given', () => {
    const hostile = [
      '/Users/reporter/Smith v Jones/witness.wav',
      'C:\\Users\\reporter\\Smith v Jones\\witness.trm',
      'witness.wav',
      'Smith_v_Jones.trm',
      '2026-11-13 Smith.wav',
    ]
    for (const value of hostile) {
      const text = buildDiagnostics({
        appVersion: value,
        os: value,
        arch: value,
        ffmpegVersion: value,
        errorType: value,
      })
      expectNoPathOrName(text)
      expect(text).toContain('App version: unknown')
      expect(text).toContain('Error type: unknown')
    }
  })

  it('reports only the error class, never its message', () => {
    expect(errorTypeOf(new TypeError('cannot read /Users/reporter/witness.wav'))).toBe('TypeError')
    expect(errorTypeOf(Object.assign(new Error('x'), { name: 'Smith v Jones.wav' }))).toBe('Error')
    expect(errorTypeOf('a string thrown at /tmp/witness.wav')).toBe('Error')
    expect(errorTypeOf(null)).toBe('Error')
  })
})

function BrokenView() {
  throw new TypeError('Cannot read /Users/reporter/Smith v Jones/witness.wav')
}

describe('error screen', () => {
  it('shows the version and copies diagnostics without the error message or any path', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    rememberEngineInfo({ ffmpegVersion: '7.1.1', os: 'macos', arch: 'aarch64' })

    render(
      <AppErrorBoundary onReload={vi.fn()}>
        <BrokenView />
      </AppErrorBoundary>,
    )

    expect(screen.getByText(`DepoAudio ${packageJson.version}`)).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Copy diagnostics' }))
    expect(await screen.findByText('Diagnostics copied. Nothing was sent.')).toBeVisible()
    expect(clipboardText).toContain(`App version: ${packageJson.version}`)
    expect(clipboardText).toContain('OS: macos (aarch64)')
    expect(clipboardText).toContain('FFmpeg: 7.1.1')
    expect(clipboardText).toContain('Error type: TypeError')
    expectNoPathOrName(clipboardText)
    expect(screen.getByText('No diagnostic information is sent automatically.')).toBeVisible()
  })

  it('explains a clipboard failure and keeps the text visible to copy by hand', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    navigator.clipboard.writeText.mockRejectedValueOnce(new Error('denied'))

    render(
      <AppErrorBoundary onReload={vi.fn()}>
        <BrokenView />
      </AppErrorBoundary>,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Copy diagnostics' }))
    expect(await screen.findByText(/couldn't copy/i)).toBeVisible()
    expect(screen.getByText(/Error type: TypeError/)).toBeInTheDocument()
  })
})

describe('Settings', () => {
  it('shows the version and copies diagnostics from the engine check', async () => {
    render(
      <SettingsPanel
        open
        onOpenChange={vi.fn()}
        prefs={{}}
        health={{ ffmpeg: true, ffmpegVersion: 'n7.1.1', os: 'windows', arch: 'x86_64' }}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Updates' }))
    expect(screen.getByText(`Version ${packageJson.version}`)).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Copy diagnostics' }))
    expect(await screen.findByText('Diagnostics copied. Nothing was sent.')).toBeVisible()
    expect(clipboardText).toBe(
      [
        'DepoAudio diagnostics',
        `App version: ${packageJson.version}`,
        'OS: windows (x86_64)',
        'FFmpeg: n7.1.1',
        'Error type: none',
      ].join('\n'),
    )
  })
})
