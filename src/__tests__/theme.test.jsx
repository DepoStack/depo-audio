import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTheme as useNextTheme } from 'next-themes'
import useTheme from '../hooks/useTheme'

vi.mock('next-themes', () => ({ useTheme: vi.fn() }))

describe('useTheme preference bridge', () => {
  const setNextTheme = vi.fn()

  beforeEach(() => {
    setNextTheme.mockReset()
    useNextTheme.mockReturnValue({ resolvedTheme: 'dark', setTheme: setNextTheme })
  })

  it('applies the backend-hydrated preference to next-themes', async () => {
    renderHook(() => useTheme('light', vi.fn()))
    await waitFor(() => expect(setNextTheme).toHaveBeenCalledWith('light'))
  })

  it('updates preference state instead of writing storage independently', () => {
    const setPreferenceTheme = vi.fn()
    const { result } = renderHook(() => useTheme('dark', setPreferenceTheme))
    setNextTheme.mockClear()

    act(() => result.current.cycleTheme())

    expect(setNextTheme).toHaveBeenCalledWith('light')
    expect(setPreferenceTheme).toHaveBeenCalledWith('light')
  })
})
