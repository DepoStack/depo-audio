import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import LibraryTab from '../components/Library/LibraryTab'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('../hooks/PreferencesContext', () => ({
  usePreferencesContext: () => ({ maxScanDepth: 7 }),
}))
vi.mock('../lib/speakerColors', () => ({
  useSpeakerColors: () => ['#111'],
  speakerColorAt: () => '#111',
}))

function catJobs() {
  return Array.from({ length: 25 }, (_, index) => {
    const number = String(index + 1).padStart(2, '0')
    const files =
      index === 0
        ? [
            { path: 'C:\\FTR\\CR24_20180621-1449_01d4096f0757ee50.trm' },
            { path: 'C:\\FTR\\CR24_20180621-1454_01d4096fbaa19b00.trm' },
          ]
        : [{ path: `C:\\FTR\\session-${number}.trm` }]
    return {
      name: `Session ${number}`,
      path: `C:\\FTR\\session-${number}`,
      software: 'FTR Gold',
      dateModified: '2026-08-05',
      files,
    }
  })
}

describe('LibraryTab court-software jobs', () => {
  afterEach(cleanup)

  beforeEach(() => {
    invoke.mockReset()
  })

  it('uses one scan depth, exposes every result through pagination, and queues the full batch', async () => {
    const jobs = catJobs()
    invoke.mockImplementation(command => {
      if (command === 'detect_cat_software_cmd') {
        return Promise.resolve([{ name: 'FTR Gold', vendor: 'For The Record', path: 'C:\\FTR', jobCount: 26 }])
      }
      if (command === 'scan_cat_jobs_cmd') return Promise.resolve(jobs)
      return Promise.resolve()
    })
    const onReexport = vi.fn()

    render(
      <LibraryTab cases={[]} setCases={vi.fn()} search="" setSearch={vi.fn()} labels={[]} onReexport={onReexport} />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Find court software' }))

    await waitFor(() => expect(screen.getByText('Showing 20 of 25')).toBeInTheDocument())
    expect(invoke).toHaveBeenCalledWith('detect_cat_software_cmd', { maxDepth: 7 })
    expect(invoke).toHaveBeenCalledWith('scan_cat_jobs_cmd', { path: 'C:\\FTR', maxDepth: 7 })
    expect(screen.getByText('26 files')).toBeInTheDocument()
    expect(screen.getByText('Session 20')).toBeInTheDocument()
    expect(screen.queryByText('Session 21')).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Show 5 more' }))
    expect(screen.getByText('25 found')).toBeInTheDocument()
    expect(screen.getByText('Session 25')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: /Session 01/ }))
    expect(onReexport).toHaveBeenCalledWith(
      ['C:\\FTR\\CR24_20180621-1449_01d4096f0757ee50.trm', 'C:\\FTR\\CR24_20180621-1454_01d4096fbaa19b00.trm'],
      'Session 01',
    )
  })
})
