import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { cwd } from 'node:process'
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { MODES, DEPOAUDIO_RELEASES_URL, DEPOSTACK_URL } from '../constants'
import FileRow from '../components/Convert/FileRow'
import UpdateBanner from '../components/UpdateBanner'
import Spinner from '../components/common/Spinner'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: path => `asset://${path}`,
  invoke: vi.fn(),
}))

const readProjectFile = relativePath => readFileSync(resolve(cwd(), relativePath), 'utf8')

afterEach(cleanup)

describe('workflow remediation contract', () => {
  it('uses channel and microphone language rather than inferred speaker identity', () => {
    const convertSource = readProjectFile('src/components/Convert/ConvertTab.jsx')
    const settingsSource = readProjectFile('src/components/SettingsPanel.jsx')

    expect(MODES.find(mode => mode.id === 'split')?.label).toBe('Split Channels')
    expect(convertSource).toContain('Balance Microphone Levels')
    expect(settingsSource).toContain("{ value: 'split', label: 'Split Channels' }")
    expect(`${convertSource}\n${settingsSource}`).not.toMatch(/Split by Speaker|Balance Speaker Volume/)
  })

  it('keeps startup free of Google font requests and allowlists only exact trust URLs', () => {
    const indexHtml = readProjectFile('index.html')
    const capability = JSON.parse(readProjectFile('src-tauri/capabilities/default.json'))
    const opener = capability.permissions.find(permission => permission.identifier === 'opener:allow-open-url')

    expect(indexHtml).not.toMatch(/fonts\.(?:googleapis|gstatic)\.com/i)
    expect(opener.allow).toEqual([{ url: DEPOSTACK_URL }, { url: DEPOAUDIO_RELEASES_URL }])
    expect(opener.allow.some(entry => /\*/.test(entry.url))).toBe(false)
  })

  it('exposes determinate update and conversion progress to assistive technology', () => {
    const updater = {
      status: 'downloading',
      progress: 0.42,
      dismissed: false,
      installUpdate: vi.fn(),
      dismiss: vi.fn(),
    }
    const updateView = render(<UpdateBanner updater={updater} />)

    expect(screen.getByRole('status')).toHaveTextContent('Downloading update…')
    expect(screen.getByText('42%')).toHaveAttribute('aria-hidden', 'true')
    expect(screen.getByRole('progressbar', { name: 'Update download progress' })).toHaveAttribute('aria-valuenow', '42')
    updateView.unmount()

    render(
      <FileRow
        file={{ name: 'hearing.wav', path: 'C:\\case\\hearing.wav', fmt: null }}
        job={{ status: 'converting', phase: 'encoding', seconds: 30, total: 60 }}
        converting
        onRemove={vi.fn()}
      />,
    )

    expect(screen.getByText('Encoding… 50%')).toBeVisible()
    expect(screen.getByRole('progressbar', { name: 'Conversion progress for hearing.wav' })).toHaveAttribute(
      'aria-valuenow',
      '50',
    )
  })

  it('marks the reusable spinner as decorative by default', () => {
    const { container } = render(<Spinner />)
    expect(container.querySelector('svg')).toHaveAttribute('aria-hidden', 'true')
  })
})
