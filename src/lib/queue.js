import { ftrRecordingDate, ftrSessionKey, sortRecordingChunks } from '../utils'

export function partitionQueuePaths(inputs = []) {
  const uniquePaths = [
    ...new Set(
      inputs.map(input => (typeof input === 'string' ? input : input?.path)).filter(path => typeof path === 'string'),
    ),
  ]

  return {
    queuePaths: uniquePaths.filter(path => !path.toLowerCase().endsWith('.trs')),
    trsCompanions: uniquePaths.filter(path => path.toLowerCase().endsWith('.trs')),
  }
}

export function mergeQueuedFiles(current, incoming, { replace = false } = {}) {
  const combined = replace ? [] : [...current]
  const seen = new Set(combined.map(file => file.path))

  for (const file of incoming) {
    if (!file?.path || seen.has(file.path)) continue
    seen.add(file.path)
    combined.push(file)
  }

  return sortRecordingChunks(combined, file => file.path)
}

export function catJobPaths(job) {
  return (job?.files || []).map(file => file?.path).filter(Boolean)
}

const countFiles = n => `${n} ${n === 1 ? 'file' : 'files'}`

// Copy for the dialog shown when a Library re-export meets a non-empty Convert
// queue. It is computed from the queue as it is now, so a file dropped while
// the dialog is open is counted (and named as added) before Replace removes it.
export function reexportConfirmCopy({ queued, queuedAtOpen, incoming }) {
  const libraryFiles = incoming === 1 ? 'the Library file' : `the ${incoming} Library files`
  if (queued === 0) {
    return {
      title: 'The Convert queue is now empty',
      description: `Nothing is waiting in Convert any more, so ${libraryFiles} can be queued without replacing anything.`,
      confirmLabel: 'Queue',
      destructive: false,
      secondaryLabel: null,
    }
  }
  const waiting = queued === 1 ? 'The file waiting in Convert is' : `The ${queued} files waiting in Convert are`
  const added = queued - queuedAtOpen
  const addedNote =
    added > 0
      ? ` ${added === 1 ? '1 file was' : `${added} files were`} added to the queue while this was open and ${added === 1 ? 'is' : 'are'} counted here.`
      : ''
  return {
    title: `Replace the ${countFiles(queued)} in the queue?`,
    description: `${waiting} taken out of the queue. Nothing is deleted from disk. Add to queue keeps ${queued === 1 ? 'it' : 'them'} and adds ${libraryFiles} after ${queued === 1 ? 'it' : 'them'}.${addedNote}`,
    confirmLabel: `Replace ${countFiles(queued)}`,
    destructive: true,
    secondaryLabel: 'Add to queue',
  }
}

// Plain notice for a queued file with more than one audio track. `info` is
// probe_audio_streams_cmd's answer: every audio stream in file order and the
// index of the one DepoAudio converts (the same stream the export record
// names). One track, or no answer, means no notice.
export function audioTrackNotice(info) {
  const streams = Array.isArray(info?.streams) ? info.streams : []
  if (streams.length < 2) return ''
  const index = Number.isInteger(info.convertedIndex) ? info.convertedIndex : 0
  const chosen = streams[index] || {}
  const tags = [chosen.language, chosen.title ? `"${chosen.title}"` : null].filter(Boolean)
  const others = streams.length - 1
  return `This file has ${streams.length} audio tracks; converting track ${index + 1}${
    tags.length ? ` (${tags.join(', ')})` : ''
  }. The other ${others === 1 ? 'track is' : `${others} tracks are`} not converted.`
}

// FTR recordings (.trm/.ftr/.tll). `info.trm` is what reading the file found
// (probe_audio_streams_cmd): whether the recorder closed it, and the session
// flags in its header. No answer means no notice.
// The session marks come from header flags read as described in notes that
// have not yet been checked against a real recording, so the notices say what
// was (not) found, never that a file is missing.
export const TRM_OPEN_ENDED_NOTICE =
  "This recording's header says it was never closed (or is still recording); DepoAudio read the audio written so far."
export const TRM_SESSION_FIRST_MISSING =
  "No queued file is marked as this session's first file; if the session has earlier files, add them too."
export const TRM_SESSION_LAST_MISSING =
  "No queued file is marked as this session's last file; if the session has later files, add them too."

export function trmRecordingNotice(info) {
  return info?.trm?.openEnded ? TRM_OPEN_ENDED_NOTICE : ''
}

const DAY_MS = 24 * 60 * 60 * 1000
const dateMs = date => Date.UTC(Number(date.slice(0, 4)), Number(date.slice(4, 6)) - 1, Number(date.slice(6, 8)))

// Queued chunks of one place (folder and location in the canonical FTR name),
// split into runs of consecutive recording dates, so a session that runs past
// midnight stays one group. Two sessions recorded at one place on the same
// day (or on consecutive days) share a group, so one session's marks can hide
// the other's missing end: this can under-report, never over-report.
function trmSessionGroups(files) {
  const places = new Map()
  for (const file of files || []) {
    const key = ftrSessionKey(file?.path)
    if (!key) continue
    if (!places.has(key)) places.set(key, [])
    places.get(key).push(file.path)
  }
  const groups = []
  for (const paths of places.values()) {
    const dates = [...new Set(paths.map(ftrRecordingDate))].sort()
    const runOf = new Map()
    let run = -1
    dates.forEach((date, index) => {
      if (index === 0 || dateMs(date) - dateMs(dates[index - 1]) !== DAY_MS) run += 1
      runOf.set(date, run)
    })
    const runs = []
    for (const path of paths) {
      const index = runOf.get(ftrRecordingDate(path))
      if (!runs[index]) runs[index] = []
      runs[index].push(path)
    }
    groups.push(...runs.filter(Boolean))
  }
  return groups
}

// Session ends: queued chunks of one session (see trmSessionGroups) where none
// is flagged as the session's first file, or none as its last. Only once
// every chunk of the group has been read, so nothing is claimed about a file
// that was not. The notice goes on the group's first queued file. Returns
// { [path]: notice }.
export function trmSessionNotices(files, audioStreams) {
  const notices = {}
  for (const paths of trmSessionGroups(files)) {
    const routes = paths.map(path => audioStreams?.[path]?.trm)
    if (routes.some(route => !route)) continue
    const parts = []
    if (!routes.some(route => route.sessionFirst)) parts.push(TRM_SESSION_FIRST_MISSING)
    if (!routes.some(route => route.sessionLast)) parts.push(TRM_SESSION_LAST_MISSING)
    if (parts.length > 0) notices[paths[0]] = parts.join(' ')
  }
  return notices
}
