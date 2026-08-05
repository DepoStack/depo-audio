import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import Waveform from '../components/common/Waveform'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

class ResizeObserverStub {
  observe() {}
  disconnect() {}
}

describe('Waveform seek and request lifecycle', () => {
  beforeEach(() => {
    vi.stubGlobal('ResizeObserver', ResizeObserverStub)
    invoke.mockReset()
    invoke.mockImplementation(command =>
      command === 'waveform_peaks_cmd' ? new Promise(() => {}) : Promise.resolve(false),
    )
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('makes the whole truthful slider surface clickable using its own bounds', () => {
    const onSeek = vi.fn()
    render(<Waveform audioPath="C:\\audio\\hearing.wav" duration={100} currentTime={25} onSeek={onSeek} />)
    const slider = screen.getByRole('slider', { name: /audio waveform seek control/i })
    slider.getBoundingClientRect = () => ({ left: 100, width: 200, right: 300, top: 0, bottom: 48, height: 48 })

    fireEvent.click(slider.firstElementChild, { clientX: 150 })
    expect(onSeek).toHaveBeenCalledWith(25)

    fireEvent.keyDown(slider, { key: 'ArrowRight' })
    expect(onSeek).toHaveBeenLastCalledWith(30)
    fireEvent.keyDown(slider, { key: 'End' })
    expect(onSeek).toHaveBeenLastCalledWith(100)
  })

  it('does not claim slider semantics without a known positive duration', () => {
    render(<Waveform audioPath="C:\\audio\\hearing.wav" duration={0} onSeek={vi.fn()} />)
    expect(screen.queryByRole('slider')).not.toBeInTheDocument()
    expect(screen.getByRole('img', { name: /audio waveform/i })).not.toHaveAttribute('aria-valuenow')
  })

  it('cancels the same per-request id when an in-flight view is removed', () => {
    const { unmount } = render(<Waveform audioPath="C:\\audio\\hearing.wav" duration={100} />)
    const peakCall = invoke.mock.calls.find(([command]) => command === 'waveform_peaks_cmd')
    expect(peakCall?.[1]?.requestId).toBeTruthy()

    unmount()

    expect(invoke).toHaveBeenCalledWith('cancel_waveform_cmd', { requestId: peakCall[1].requestId })
  })
})
