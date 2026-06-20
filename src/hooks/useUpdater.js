import { useState, useEffect, useRef, useCallback } from 'react'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

// ── Auto-update via GitHub Releases ─────────────────────────────────────────
//
// On launch, asks the updater plugin whether a newer signed release exists
// (the endpoint is the repo's latest.json). If one does, the UI surfaces a
// banner; the user chooses when to download + install, after which the app
// relaunches into the new version.
//
// In dev or a plain browser there is no updater backend, so check() throws and
// we fail silent (status stays idle, no banner) unless the user checked manually.

export default function useUpdater() {
  const [update, setUpdate] = useState(null)   // { version, currentVersion, body, date }
  const [status, setStatus] = useState('idle') // idle | checking | available | downloading | ready | uptodate | error
  const [progress, setProgress] = useState(0)  // 0..1
  const [error, setError] = useState(null)
  const updateRef = useRef(null)               // the raw Update handle from the plugin

  const checkForUpdate = useCallback(async (manual = false) => {
    setStatus('checking')
    setError(null)
    try {
      const u = await check()
      if (u) {
        updateRef.current = u
        setUpdate({ version: u.version, currentVersion: u.currentVersion, body: u.body, date: u.date })
        setStatus('available')
      } else {
        updateRef.current = null
        setUpdate(null)
        setStatus(manual ? 'uptodate' : 'idle')
      }
    } catch (e) {
      // No updater backend (dev/browser) or a network/signature error.
      setStatus(manual ? 'error' : 'idle')
      setError(String(e))
    }
  }, [])

  const installUpdate = useCallback(async () => {
    const u = updateRef.current
    if (!u) return
    setStatus('downloading')
    setProgress(0)
    try {
      let total = 0
      let received = 0
      await u.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          total = event.data?.contentLength || 0
        } else if (event.event === 'Progress') {
          received += event.data?.chunkLength || 0
          if (total > 0) setProgress(Math.min(received / total, 1))
        } else if (event.event === 'Finished') {
          setProgress(1)
        }
      })
      setStatus('ready')
      await relaunch()
    } catch (e) {
      setStatus('error')
      setError(String(e))
    }
  }, [])

  const dismiss = useCallback(() => {
    // Hide the banner for this session; don't forget the update so Settings
    // can still show it's available.
    setStatus(s => (s === 'available' ? 'idle' : s))
  }, [])

  // Quietly check once on launch. Deferred to a microtask so the first
  // status update doesn't run synchronously inside the effect body.
  useEffect(() => {
    let cancelled = false
    Promise.resolve().then(() => { if (!cancelled) checkForUpdate(false) })
    return () => { cancelled = true }
  }, [checkForUpdate])

  return { update, status, progress, error, checkForUpdate, installUpdate, dismiss }
}
