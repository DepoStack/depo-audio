import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'

import MergeTab, { addMergeSources, MAX_MERGE_SOURCES, overLengthMessage } from '../components/Merge/MergeTab'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

const durations = {}
let progressHandler = null
let finishMerge = null

beforeEach(() => {
  for (const key of Object.keys(durations)) delete durations[key]
  progressHandler = null
  finishMerge = null
  listen.mockImplementation(async (_event, handler) => {
    progressHandler = handler
    return vi.fn()
  })
  invoke.mockImplementation((command, args) => {
    if (command === 'probe_audio_streams_cmd') {
      return Promise.resolve({ durationSecs: durations[args.path] ?? 60 })
    }
    if (command === 'merge_audio_cmd') {
      return new Promise((resolve, reject) => {
        finishMerge = { resolve, reject }
      })
    }
    if (command === 'merge_cancel_cmd') return Promise.resolve(true)
    return Promise.resolve(null)
  })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

async function addFiles(paths) {
  open.mockResolvedValueOnce(paths)
  const add = screen.queryByRole('button', { name: 'Add Files' })
  fireEvent.click(add)
  await waitFor(() => expect(open).toHaveBeenCalled())
}

describe('addMergeSources', () => {
  it('stops at four recordings and says how many were left out', () => {
    const paths = ['/a.wav', '/b.wav', '/c.wav', '/d.wav', '/e.wav', '/f.wav']
    const next = addMergeSources([], paths)
    expect(next.sources).toHaveLength(MAX_MERGE_SOURCES)
    expect(next.notice).toBe('Merge takes up to 4 recordings, so 2 more were not added.')
  })

  it('refuses a recording already in the list', () => {
    const next = addMergeSources([{ path: '/a.wav', name: 'a.wav' }], ['/a.wav', '/b.wav'])
    expect(next.sources.map(s => s.path)).toEqual(['/a.wav', '/b.wav'])
    expect(next.added.map(s => s.path)).toEqual(['/b.wav'])
    expect(next.notice).toBe('a.wav is already in the list.')
  })
})

describe('MergeTab', () => {
  it('names a recording over 30 minutes before Merge starts and keeps Merge off', async () => {
    durations['/long.wav'] = 31 * 60
    render(<MergeTab />)
    await addFiles(['/mic.wav', '/long.wav'])

    expect(await screen.findByText(overLengthMessage('long.wav'))).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Merge' })).toBeDisabled()
    expect(invoke).not.toHaveBeenCalledWith('merge_audio_cmd', expect.anything())
  })

  it('turns Add Files off at four recordings', async () => {
    render(<MergeTab />)
    await addFiles(['/a.wav', '/b.wav', '/c.wav', '/d.wav'])
    await waitFor(() => expect(screen.getByRole('button', { name: 'Add Files' })).toBeDisabled())
    expect(screen.getByText('Four recordings is the most one merge takes.')).toBeInTheDocument()
  })

  it('describes the two strategies as they work', async () => {
    render(<MergeTab />)
    await addFiles(['/a.wav', '/b.wav'])
    expect(await screen.findByText('Loudest source')).toBeInTheDocument()
    expect(screen.getByText(/loudest for each half second, with a short crossfade/)).toBeInTheDocument()
    expect(screen.getByText('Averages every recording that is playing at each moment')).toBeInTheDocument()
    expect(screen.getByRole('radio', { name: /Loudest source/ })).toBeChecked()
  })

  it('shows progress and cancels the running merge', async () => {
    render(<MergeTab />)
    await addFiles(['/a.wav', '/b.wav'])
    await waitFor(() => expect(screen.getByRole('button', { name: 'Merge' })).toBeEnabled())

    fireEvent.click(screen.getByRole('button', { name: 'Merge' }))
    await waitFor(() => expect(finishMerge).not.toBeNull())
    expect(invoke).toHaveBeenCalledWith('merge_audio_cmd', {
      job: expect.objectContaining({ sources: ['/a.wav', '/b.wav'], strategy: 'loudest', rate: '48000' }),
    })

    act(() => progressHandler({ payload: { stage: 'merging', percent: 42 } }))
    expect(screen.getByRole('progressbar', { name: 'Merge progress' })).toHaveAttribute('aria-valuenow', '42')
    expect(screen.getByText('Merging 42%')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('merge_cancel_cmd'))

    await act(async () => finishMerge.reject('Merge cancelled'))
    expect(await screen.findByText('Cancelled. Nothing was saved.')).toBeInTheDocument()
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument()
  })

  it('stops between sync checks when Cancel comes before the next one starts', async () => {
    let finishFirst = null
    invoke.mockImplementation((command, args) => {
      if (command === 'probe_audio_streams_cmd') return Promise.resolve({ durationSecs: 60 })
      if (command === 'detect_sync_cmd') {
        return new Promise(resolve => {
          finishFirst = () => resolve({ offsetSeconds: 0.5, confidence: 0.9, isSameEvent: true })
        })
      }
      // Between two checks no job holds the slot, so there is nothing to stop.
      if (command === 'merge_cancel_cmd') return Promise.resolve(false)
      return Promise.resolve(null)
    })
    render(<MergeTab />)
    await addFiles(['/a.wav', '/b.wav', '/c.wav'])
    fireEvent.click(await screen.findByRole('button', { name: 'Check Sync' }))
    await waitFor(() => expect(finishFirst).not.toBeNull())

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await act(async () => finishFirst())

    expect(await screen.findByText('Cancelled. Nothing was saved.')).toBeInTheDocument()
    expect(invoke.mock.calls.filter(([command]) => command === 'detect_sync_cmd')).toHaveLength(1)
  })
})
