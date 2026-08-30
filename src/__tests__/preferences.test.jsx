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

  it('clears stale learned-model preferences instead of hydrating them', async () => {
    invoke.mockImplementation(command =>
      command === 'prefs_get'
        ? Promise.resolve({
            mode: 'stereo',
            format: 'wav',
            denoise: true,
            denoiseQuality: 'best',
            enhance: true,
            dereverb: true,
          })
        : Promise.resolve(true),
    )

    const { result, unmount } = renderHook(() => usePreferences())
    await waitFor(() => expect(result.current.prefsReady).toBe(true))
    await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === 'prefs_set')).toBe(true), {
      timeout: 2000,
    })

    const savedPatch = invoke.mock.calls.find(([command]) => command === 'prefs_set')[1].patch
    expect(savedPatch).toMatchObject({ denoise: false, denoiseQuality: 'fast', enhance: false, dereverb: false })
    expect(result.current).not.toHaveProperty('setDenoise')
    expect(result.current).not.toHaveProperty('setEnhance')
    expect(result.current).not.toHaveProperty('setDereverb')
    unmount()
  })
})

describe('processing defaults', () => {
  it('never enables learned-model processing in presets', () => {
    for (const preset of PRESETS) {
      expect(preset.settings).toMatchObject({ denoise: false, enhance: false, dereverb: false })
    }
  })

  it('fails closed even when stale preset data requests learned processing', () => {
    const courtroom = PRESETS.find(preset => preset.id === 'courtroom')
    expect(
      resolvePresetSettings({ ...courtroom.settings, denoise: true, enhance: true, dereverb: true }),
    ).toMatchObject({ denoise: false, enhance: false, dereverb: false })
    expect(resolvePresetSettings({ ...courtroom.settings, mode: 'keep' }).autoLevel).toBe(false)
  })

  it('keeps preset descriptions within released non-learned processing', () => {
    const courtroom = PRESETS.find(preset => preset.id === 'courtroom')
    expect(PRESETS.map(preset => preset.desc).join(' ')).not.toMatch(
      /denoise|noise removal|enhance clarity|echo|reverb/i,
    )
    expect(courtroom.desc).toMatch(/microphone channels/i)
  })
})
