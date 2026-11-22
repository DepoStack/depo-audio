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

// Plain notice for a queued DigitalCAT .dm, from probe_audio_streams_cmd's
// `dm` answer (read from the file's own block headers): how many
// recorded-over blocks are skipped, whether stamps go backwards somewhere
// (only that is known, not why), whether reading stopped at a short block
// before the end (as SoundSync's player does) or the last block is short, and
// whether the file ends partway through a block. Nothing to say, or no
// answer, means no notice.
export function dmNotice(info) {
  const dm = info?.dm
  if (!dm || typeof dm !== 'object') return ''
  const count = value => (Number.isInteger(value) && value > 0 ? value : 0)
  const erased = count(dm.blocksErased)
  const afterShort = count(dm.bytesAfterShortBlock)
  const parts = [
    erased ? `${erased} recorded-over ${erased === 1 ? 'block' : 'blocks'} skipped.` : '',
    dm.stampsMonotonic === false
      ? 'Time stamps go backwards in places; the file does not say why. Audio is kept in file order.'
      : '',
    afterShort
      ? `A short block comes before the end of the file. The format notes say SoundSync's player stops there, and so does DepoAudio: the ${afterShort === 1 ? 'byte' : `${afterShort} bytes`} after it ${afterShort === 1 ? 'is' : 'are'} not converted.`
      : dm.shortFinalBlock === true
        ? "The last block is shorter than the others. The format notes say SoundSync's player does not play it; its whole codec blocks are converted."
        : '',
    count(dm.truncatedTailBytes)
      ? 'The file ends partway through a block, so it may not have finished recording or copying; the complete blocks are converted and the incomplete one is left out.'
      : '',
  ]
  return parts.filter(Boolean).join(' ')
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
// (probe_audio_streams_cmd): whether the recorder closed it, its boundary
// flags, whether it carries a signature, and its codec; `info.trmRefusal` is
// why DepoAudio will not convert it. No answer means no notice.
// The layout comes from the owner's interoperability notes, checked against a
// sample made by the owner's rebuild tool, not against a recording made by FTR,
// so the notices say what was (not) found, never that a file is missing.
export const TRM_OPEN_ENDED_NOTICE =
  "This recording's header says it was never closed (or is still recording); DepoAudio read the audio written so far."
export const TRM_SIGNED_NOTICE =
  'This recording carries a signature: signed (not verified). DepoAudio does not check signatures.'
export const TRM_START_BOUNDARY_MISSING =
  'The earliest queued file of this recording has no start-boundary mark, so FTR would accept earlier files before it; if there are any, add them too.'
export const TRM_END_BOUNDARY_MISSING =
  'The latest queued file of this recording has no end-boundary mark, so FTR would accept later files after it; if there are any, add them too.'

const TRM_PLAIN_CODECS = { mp2: 'MPEG audio layer 2', mp3: 'MP3', wma: 'Windows Media Audio' }

export function trmRecordingNotice(info) {
  if (info?.trmRefusal) return String(info.trmRefusal)
  const trm = info?.trm
  if (!trm) return ''
  const parts = []
  if (trm.openEnded) parts.push(TRM_OPEN_ENDED_NOTICE)
  const plain = TRM_PLAIN_CODECS[trm.audioCodec]
  if (plain) parts.push(`This FTR recording holds ${plain} audio, which FFmpeg reads as ordinary audio.`)
  if (trm.signature) parts.push(TRM_SIGNED_NOTICE)
  return parts.join(' ')
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

// FTR's boundary flags: a start boundary means FTR joins no earlier file to
// this one, an end boundary that it joins no later file. Within one place and
// run of dates (see trmSessionGroups), queued files in recording order
// (canonical names: by the FILETIME in the name) are cut into recordings at
// those marks. A recording whose earliest queued file has no start boundary
// could have earlier files, and one whose latest has no end boundary later
// ones. Only once every file of the group has been read, so nothing is claimed
// about a file that was not. The notice goes on the recording's earliest
// queued file. Returns { [path]: notice }.
export function trmSessionNotices(files, audioStreams) {
  const notices = {}
  for (const group of trmSessionGroups(files)) {
    const paths = sortRecordingChunks(group)
    const routes = paths.map(path => audioStreams?.[path]?.trm)
    if (routes.some(route => !route)) continue
    const recordings = []
    paths.forEach((path, index) => {
      const route = routes[index]
      const previous = recordings[recordings.length - 1]
      if (!previous || route.startBoundary || previous.closed) recordings.push({ paths: [], routes: [] })
      const current = recordings[recordings.length - 1]
      current.paths.push(path)
      current.routes.push(route)
      current.closed = Boolean(route.endBoundary)
    })
    for (const recording of recordings) {
      const parts = []
      if (!recording.routes[0].startBoundary) parts.push(TRM_START_BOUNDARY_MISSING)
      if (!recording.routes[recording.routes.length - 1].endBoundary) parts.push(TRM_END_BOUNDARY_MISSING)
      if (parts.length > 0) notices[recording.paths[0]] = parts.join(' ')
    }
  }
  return notices
}
