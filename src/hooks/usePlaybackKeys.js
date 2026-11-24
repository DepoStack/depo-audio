import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { register, unregisterAll } from '@tauri-apps/plugin-global-shortcut'
import { createPlaybackKeyDispatcher, normalizePlaybackKeys, PLAYBACK_KEY_ACTIONS } from '../lib/playbackKeys'

// Registers the person's playback keys system-wide while the setting is on.
//
// - Off (the default): nothing is registered.
// - On: every assigned key is registered on its own, so a key the system or
//   another app refuses is reported for that key and the rest still work.
// - While the key recorder in Settings is listening, every key is released so
//   the recorder can see the press, then registered again.
// - Turning the setting off, closing the window (unmount) and quitting the app
//   (Rust, RunEvent::Exit) release every key.
// - Keys stay registered while a DepoAudio text field has focus, because
//   releasing and re-taking keys on every focus change would let another app
//   take a key in between. The recorder refuses keys that type or edit in
//   common cases (bare letters, Ctrl+Alt, which is AltGr on Windows, and
//   Ctrl/Cmd with editing and cursor keys); an assigned key does not reach any
//   app, DepoAudio's text fields included, while the setting is on.
// - Every sync first forgets held keys, because a release that arrives after
//   unregisterAll() is never delivered.
//
// Each sync starts with unregisterAll(), so a page reload (which keeps the
// Rust-side registrations) never leaves a stale key behind.
export default function usePlaybackKeys(rawSettings, { registerKey = register, releaseAll = unregisterAll } = {}) {
  const settings = useMemo(() => normalizePlaybackKeys(rawSettings), [rawSettings])
  const settingsKey = JSON.stringify(settings)
  const [suspended, setSuspended] = useState(false)
  // { [action]: { accelerator, state: 'registered' | 'failed', error? } }
  const [registration, setRegistration] = useState({})
  const [syncError, setSyncError] = useState('')
  const [status, setStatus] = useState('')
  // Holds the Player's actions (null while the Player is not open) and the
  // hold mode; the key handler reads them at press time
  const [dispatcher] = useState(() => createPlaybackKeyDispatcher({ onStatus: setStatus }))
  useEffect(() => {
    dispatcher.setHoldToPlay(settings.holdToPlay)
  }, [dispatcher, settings.holdToPlay])

  // Register and release one sync at a time, in order.
  const queueRef = useRef(Promise.resolve())
  useEffect(() => {
    const wanted = JSON.parse(settingsKey)
    const active = wanted.enabled && !suspended
    let cancelled = false
    queueRef.current = queueRef.current.then(async () => {
      dispatcher.reset()
      try {
        await releaseAll()
      } catch (error) {
        if (!cancelled) setSyncError(`Playback keys could not be released: ${String(error)}`)
        return
      }
      if (!active) {
        if (!cancelled) {
          setRegistration({})
          setSyncError('')
        }
        return
      }
      const next = {}
      for (const { id } of PLAYBACK_KEY_ACTIONS) {
        const accelerator = wanted.bindings[id]
        if (!accelerator) continue
        try {
          await registerKey(accelerator, event => dispatcher.handle(id, event?.state))
          next[id] = { accelerator, state: 'registered' }
        } catch (error) {
          next[id] = { accelerator, state: 'failed', error: String(error) }
        }
      }
      if (!cancelled) {
        setRegistration(next)
        setSyncError('')
      }
    })
    return () => {
      cancelled = true
    }
  }, [settingsKey, suspended, registerKey, releaseAll, dispatcher])

  // Release everything when the app shell unmounts.
  useEffect(
    () => () => {
      queueRef.current = queueRef.current.then(() => releaseAll()).catch(() => {})
    },
    [releaseAll],
  )

  const setTarget = useCallback(
    target => {
      dispatcher.setTarget(target)
      if (target) setStatus('')
    },
    [dispatcher],
  )

  return {
    settings,
    registration,
    syncError,
    status,
    suspended,
    setSuspended,
    setTarget,
    dispatch: dispatcher.handle,
  }
}
