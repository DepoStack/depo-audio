import { useEffect } from 'react'

// ── Global keyboard shortcuts ───────────────────────────────────────────────
//
// Space        — Play / Pause (when not in an input)
// Ctrl/Cmd+E   — Start conversion
// Left/Right   — Skip prev/next track (Player tab)
// Ctrl/Cmd+1-4 — Switch tabs
// Escape       — Clear selection / close modal

export default function useKeyboard({ onPlayPause, onConvert, onSkip, onSwitchTab }) {
  useEffect(() => {
    const handler = (e) => {
      // Don't capture when typing in inputs
      const tag = e.target.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return

      const mod = e.metaKey || e.ctrlKey

      switch (e.code) {
        case 'Space':
          e.preventDefault()
          onPlayPause?.()
          break

        case 'KeyE':
          if (mod) {
            e.preventDefault()
            onConvert?.()
          }
          break

        case 'ArrowLeft':
          if (!mod) onSkip?.(-1)
          break

        case 'ArrowRight':
          if (!mod) onSkip?.(1)
          break

        case 'Digit1':
          if (mod) { e.preventDefault(); onSwitchTab?.('convert') }
          break
        case 'Digit2':
          if (mod) { e.preventDefault(); onSwitchTab?.('player') }
          break
        case 'Digit3':
          if (mod) { e.preventDefault(); onSwitchTab?.('merge') }
          break
        case 'Digit4':
          if (mod) { e.preventDefault(); onSwitchTab?.('library') }
          break
      }
    }

    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onPlayPause, onConvert, onSkip, onSwitchTab])
}
