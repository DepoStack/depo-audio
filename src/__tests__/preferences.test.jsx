import { beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook, waitFor } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
import usePreferences from '../hooks/usePreferences'
import { PRESETS, resolvePresetSettings } from '../presets'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

describe('usePreferences hydration', () => {
  beforeEach(() => invoke.mockReset())

  it('preserves empty remember-last sentinels while applying the last used mode and format', async () => {
    invoke.mockImplementation(command =>
      command === 'prefs_get'
        ? Promise.resolve({
            mode: 'split',
            format: 'mp3',
            theme: 'dark',
            defaultOutputFormat: '',
            defaultOutputMode: '',
          })
        : Promise.resolve(true),
    )

    const { result, unmount } = renderHook(() => usePreferences())
    await waitFor(() => expect(result.current.prefsReady).toBe(true))

    expect(result.current.defaultOutputFormat).toBe('')
    expect(result.current.defaultOutputMode).toBe('')
    expect(result.current.formatOut).toBe('mp3')
    expect(result.current.mode).toBe('split')
    expect(result.current.themePref).toBe('dark')
    unmount()
  })

  it('enters protected read-only preference mode when persisted data cannot be loaded', async () => {
    invoke.mockRejectedValueOnce(new Error('corrupt preferences'))

    const { result, unmount } = renderHook(() => usePreferences())
    await waitFor(() => expect(result.current.prefsReady).toBe(true))

    expect(result.current.prefsError).toContain('Saving is disabled')
    expect(invoke.mock.calls.map(([command]) => command)).toEqual(['prefs_get'])
    unmount()
  })

  it('turns auto-level off when switching to keep-channel mode', async () => {
    invoke.mockImplementation(command =>
      command === 'prefs_get'
        ? Promise.resolve({ mode: 'stereo', format: 'wav', autoLevel: true })
        : Promise.resolve(true),
    )

    const { result, unmount } = renderHook(() => usePreferences())
    await waitFor(() => expect(result.current.prefsReady).toBe(true))
    expect(result.current.autoLevel).toBe(true)

    act(() => result.current.setMode('keep'))
    expect(result.current.mode).toBe('keep')
    expect(result.current.autoLevel).toBe(false)

    act(() => result.current.setAutoLevel(true))
    expect(result.current.autoLevel).toBe(false)
    unmount()
  })

  it('surfaces theme persistence failures through the shared preference error', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    let failSave
    const saveAttempt = {
      catch: handler => {
        failSave = handler
        return Promise.resolve()
      },
    }
    invoke.mockImplementation(command =>
      command === 'prefs_get' ? Promise.resolve({ mode: 'stereo', format: 'wav', theme: 'dark' }) : saveAttempt,
    )

    const { result, unmount } = renderHook(() => usePreferences())
    await waitFor(() => expect(result.current.prefsReady).toBe(true))

    act(() => result.current.setThemePref('light'))

    await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === 'prefs_set')).toBe(true))
    act(() => failSave(new Error('disk full')))
    await waitFor(() => expect(result.current.prefsError).toContain('disk full'), { timeout: 2000 })
    expect(invoke).toHaveBeenCalledWith(
      'prefs_set',
      expect.objectContaining({ patch: expect.objectContaining({ theme: 'light' }) }),
    )
    unmount()
  })
})

describe('processing defaults', () => {
  it('only offers the production-ready fast denoise path in presets', () => {
    expect(PRESETS.every(preset => preset.settings.denoiseQuality === 'fast')).toBe(true)
  })

  it('fails closed for unavailable preset processing', () => {
    const courtroom = PRESETS.find(preset => preset.id === 'courtroom')
    expect(resolvePresetSettings(courtroom.settings, { dereverbAvailable: false }).dereverb).toBe(false)
    expect(resolvePresetSettings(courtroom.settings, { dereverbAvailable: true }).dereverb).toBe(true)
    expect(resolvePresetSettings({ ...courtroom.settings, mode: 'keep' }, { dereverbAvailable: true }).autoLevel).toBe(
      false,
    )
  })

  it('keeps the Courtroom description truthful when dereverb is unavailable', () => {
    const courtroom = PRESETS.find(preset => preset.id === 'courtroom')
    expect(courtroom.desc).not.toMatch(/echo|reverb/i)
    expect(courtroom.desc).toMatch(/background noise/i)
  })
})
