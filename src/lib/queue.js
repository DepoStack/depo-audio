import { sortRecordingChunks } from '../utils'

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
// recorded-over blocks are skipped, and whether stamps run backwards because
// part of the file was recorded over. Nothing to say, or no answer, means no
// notice.
export function dmNotice(info) {
  const dm = info?.dm
  if (!dm || typeof dm !== 'object') return ''
  const erased = Number.isInteger(dm.blocksErased) && dm.blocksErased > 0 ? dm.blocksErased : 0
  const parts = [
    erased ? `${erased} recorded-over ${erased === 1 ? 'block' : 'blocks'} skipped.` : '',
    dm.stampsMonotonic === false ? 'Parts of this file were recorded over; audio is kept in file order.' : '',
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
