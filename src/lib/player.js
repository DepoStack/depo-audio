import { fmtTime } from '../utils'

// ── Player pure logic ────────────────────────────────────────────────────────
//
// Extracted from the Player tab so characterization tests can pin the
// behavior: what files the playlist accepts, how speed cycling steps, and
// how persisted bookmarks are validated and exported.

export const AUDIO_EXTS = ['wav','mp3','flac','opus','ogg','m4a','aac','wma','aif','aiff','sgmca','trm','ftr','bwf']
export const SPEED_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 2]

// Parse the persisted playback speed; anything off the menu falls back to 1×
export function loadSpeed(raw) {
  const v = parseFloat(raw)
  return SPEED_STEPS.includes(v) ? v : 1
}

// Step the speed up/down the menu, clamped to the ends. An unknown current
// speed is treated as 1× (index 2) before stepping.
export function cycleSpeedStep(speed, dir) {
  const idx = SPEED_STEPS.indexOf(speed)
  const next = Math.max(0, Math.min(SPEED_STEPS.length - 1, (idx < 0 ? 2 : idx) + dir))
  return SPEED_STEPS[next]
}

// Validate persisted bookmarks — corrupt storage must not crash the tab on
// every launch, so anything that isn't a well-shaped array is discarded.
export function loadBookmarks(raw) {
  try {
    const v = JSON.parse(raw || '[]')
    return Array.isArray(v) ? v.filter(b => b && typeof b.time === 'number' && typeof b.trackPath === 'string') : []
  } catch { return [] }
}

// Native drops arrive unfiltered: keep only audio files, skip paths already
// queued (duplicate keys break selection), and dedupe within the drop itself.
export function freshAudioPaths(paths, tracks) {
  const fresh = paths
    .map(p => (typeof p === 'string' ? p : p.path))
    .filter(path => AUDIO_EXTS.includes(path.split('.').pop()?.toLowerCase()))
    .filter(path => !tracks.some(t => t.path === path))
  return [...new Set(fresh)]
}

// The active track's bookmarks as "MM:SS<TAB>label" lines (for transcripts)
export function bookmarksToText(bookmarks, trackPath) {
  return bookmarks
    .filter(b => b.trackPath === trackPath)
    .sort((a, b) => a.time - b.time)
    .map(b => `${fmtTime(b.time)}\t${b.label || ''}`.trimEnd())
    .join('\n')
}
