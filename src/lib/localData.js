// ── Local data in web view storage (DA-07) ──────────────────────────────────
//
// DepoAudio keeps transcripts, bookmarks, playback speed and the theme in the
// app's web view storage (localStorage). The library and settings live in the
// app data folder and are handled by the backend (src-tauri/src/localdata.rs).
// These pure helpers collect that web data for a backup, restore it from one
// without replacing anything the person already has, and clear it.

import { TRANSCRIPT_KEY_PREFIX, loadSegments, storageKey } from './transcript'
import { loadBookmarks } from './player'

// Every fixed key DepoAudio writes. Transcripts use TRANSCRIPT_KEY_PREFIX.
export const WEB_STORAGE_KEYS = ['player-bookmarks', 'player-speed', 'depoaudio-theme']
const BOOKMARKS_KEY = 'player-bookmarks'
const MAX_TRACK_PATH_CHARS = 4096

export function isDepoAudioKey(key) {
  return typeof key === 'string' && (key.startsWith(TRANSCRIPT_KEY_PREFIX) || WEB_STORAGE_KEYS.includes(key))
}

function keysOf(storage) {
  const keys = []
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index)
    if (key != null) keys.push(key)
  }
  return keys
}

function validTrackPath(path) {
  return typeof path === 'string' && path.trim() !== '' && path.length <= MAX_TRACK_PATH_CHARS
}

function sanitizeSegments(value) {
  return Array.isArray(value) ? loadSegments(JSON.stringify(value)) : null
}

const bookmarkKey = bookmark => JSON.stringify([bookmark.trackPath, bookmark.time, bookmark.label ?? ''])

// Transcripts (non-empty only) keyed by track path, plus valid bookmarks.
export function collectWebData(storage) {
  const empty = { transcripts: {}, bookmarks: [] }
  if (!storage) return empty
  try {
    const transcripts = {}
    for (const key of keysOf(storage)) {
      if (!key.startsWith(TRANSCRIPT_KEY_PREFIX)) continue
      const path = key.slice(TRANSCRIPT_KEY_PREFIX.length)
      const segments = loadSegments(storage.getItem(key))
      if (validTrackPath(path) && segments.length > 0) transcripts[path] = segments
    }
    return { transcripts, bookmarks: loadBookmarks(storage.getItem(BOOKMARKS_KEY)) }
  } catch {
    return empty
  }
}

export function summarizeWebData(storage) {
  const { transcripts, bookmarks } = collectWebData(storage)
  return { transcripts: Object.keys(transcripts).length, bookmarks: bookmarks.length }
}

// Remove every DepoAudio key. Keys DepoAudio did not write are left alone.
export function clearWebData(storage) {
  const result = { removed: 0, failed: 0 }
  if (!storage) return result
  let keys
  try {
    keys = keysOf(storage).filter(isDepoAudioKey)
  } catch {
    return result
  }
  for (const key of keys) {
    try {
      storage.removeItem(key)
      result.removed += 1
    } catch {
      result.failed += 1
    }
  }
  return result
}

// Restore a backup's transcripts and bookmarks. A transcript is written only
// when this computer has none for that track; a different local transcript is
// kept and counted. Bookmarks are added unless the same one already exists.
export function restoreWebData(storage, backup) {
  const result = { transcriptsAdded: 0, transcriptsKept: 0, bookmarksAdded: 0, failed: 0 }
  if (!storage) return result
  const transcripts =
    backup?.transcripts && typeof backup.transcripts === 'object' && !Array.isArray(backup.transcripts)
      ? backup.transcripts
      : {}

  for (const [path, value] of Object.entries(transcripts)) {
    const segments = sanitizeSegments(value)
    if (!validTrackPath(path) || !segments || segments.length === 0) continue
    try {
      const existing = loadSegments(storage.getItem(storageKey(path)))
      if (existing.length > 0) {
        if (JSON.stringify(existing) !== JSON.stringify(segments)) result.transcriptsKept += 1
        continue
      }
      storage.setItem(storageKey(path), JSON.stringify(segments))
      result.transcriptsAdded += 1
    } catch {
      result.failed += 1
    }
  }

  const incoming = Array.isArray(backup?.bookmarks) ? loadBookmarks(JSON.stringify(backup.bookmarks)) : []
  if (incoming.length > 0) {
    try {
      const current = loadBookmarks(storage.getItem(BOOKMARKS_KEY))
      const seen = new Set(current.map(bookmarkKey))
      const added = incoming.filter(bookmark => validTrackPath(bookmark.trackPath) && !seen.has(bookmarkKey(bookmark)))
      if (added.length > 0) {
        storage.setItem(BOOKMARKS_KEY, JSON.stringify([...current, ...added]))
        result.bookmarksAdded = added.length
      }
    } catch {
      result.failed += 1
    }
  }
  return result
}

export function defaultBackupName(date = new Date()) {
  const pad = value => String(value).padStart(2, '0')
  return `DepoAudio backup ${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}.zip`
}
