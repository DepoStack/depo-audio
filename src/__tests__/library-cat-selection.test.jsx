import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import LibraryTab from '../components/Library/LibraryTab'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('../hooks/PreferencesContext', () => ({ usePreferencesContext: () => ({ maxScanDepth: 7 }) }))
vi.mock('../lib/speakerColors', () => ({
  useSpeakerColors: () => ['#111'],
  speakerColorAt: () => '#111',
}))

const sources = [
  { name: 'Source A', path: '/synthetic/a', jobCount: 1 },
  { name: 'Source B', path: '/synthetic/b', jobCount: 1 },
]
const job = name => ({
  name,
  path: `/synthetic/${name}`,
  software: name,
  files: [{ path: '/synthetic/audio.wav' }],
})
function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
function renderLibrary() {
  render(<LibraryTab cases={[]} setCases={vi.fn()} search="" setSearch={vi.fn()} labels={[]} onReexport={vi.fn()} />)
  fireEvent.click(screen.getByRole('button', { name: 'Find court software' }))
}

describe('detected court-software source selection', () => {
  afterEach(() => {
    cleanup()
    invoke.mockReset()
  })

  it('shows pending selection, retains attributed jobs, and retries a failed source', async () => {
    const pending = deferred()
    let attempt = 0
    invoke.mockImplementation((command, args) => {
      if (command === 'detect_cat_software_cmd') return Promise.resolve(sources)
      if (args.path === sources[0].path) return Promise.resolve([job('Previous session')])
      attempt += 1
      return attempt === 1 ? pending.promise : Promise.resolve([job('New session')])
    })
    renderLibrary()
    expect(await screen.findByText('Previous session')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: /Source B/ }))
    expect(screen.getByRole('button', { name: /Source B/ })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByRole('status', { name: 'Court software scan status' })).toHaveTextContent(
      'Scanning jobs in Source B',
    )
    expect(screen.getByText('Previous session')).toBeVisible()
    expect(screen.getByText('Results from Source A')).toBeVisible()
    await act(async () => pending.reject(new Error('Synthetic scan failure')))
    expect(screen.getByRole('alert')).toHaveTextContent('Could not scan jobs in Source B')
    fireEvent.click(screen.getByRole('button', { name: 'Retry Source B scan' }))
    expect(await screen.findByText('New session')).toBeVisible()
    expect(screen.queryByText('Previous session')).not.toBeInTheDocument()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('ignores a previous source completion after the next source wins', async () => {
    const first = deferred()
    invoke.mockImplementation((command, args) => {
      if (command === 'detect_cat_software_cmd') return Promise.resolve(sources)
      return args.path === sources[0].path ? first.promise : Promise.resolve([job('New session')])
    })
    renderLibrary()
    fireEvent.click(await screen.findByRole('button', { name: /Source B/ }))
    expect(await screen.findByText('New session')).toBeVisible()
    await act(async () => first.resolve([job('Old session')]))
    expect(screen.queryByText('Old session')).not.toBeInTheDocument()
    expect(screen.getByText('New session')).toBeVisible()
  })

  it('does not restore dismissed results when a pending scan settles', async () => {
    const first = deferred()
    invoke.mockImplementation(command =>
      command === 'detect_cat_software_cmd' ? Promise.resolve(sources) : first.promise,
    )
    renderLibrary()
    fireEvent.click(await screen.findByRole('button', { name: 'Dismiss court-software results' }))
    await act(async () => first.resolve([job('Late session')]))
    expect(screen.queryByText('Late session')).not.toBeInTheDocument()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Find court software' })).toBeEnabled())
  })
})
