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

// HH:MM:SS for a queue notice.
function clockTime(seconds) {
  const total = Math.max(0, Math.floor(Number(seconds) || 0))
  const pad = value => String(value).padStart(2, '0')
  return `${pad(Math.floor(total / 3600))}:${pad(Math.floor((total % 3600) / 60))}:${pad(total % 60)}`
}

// Shown in full up to this many remote-participant changes; the rest are counted.
const DCR_CHANGES_LISTED = 3

// Plain notices for a queued Liberty / BIS Digital DCR, from the `dcr`
// summary in probe_audio_streams_cmd's answer (DepoAudio's own reader, not
// ffprobe). A recording that cannot be converted gets only the reason.
// Nothing here comes from the recording's case or docket fields.
export function dcrQueueNotices(info) {
  const dcr = info?.dcr
  if (!dcr || typeof dcr !== 'object') return []
  if (dcr.problem) return [String(dcr.problem)]
  const notices = []
  const names = Array.isArray(dcr.channelNames) ? dcr.channelNames.filter(name => typeof name === 'string') : []
  if (names.length > 0) {
    notices.push(
      `Channels read from the recording: ${names.join(', ')}. Split outputs and export records use these names instead of the channel labels below.`,
    )
  }
  const changes = Array.isArray(dcr.conferenceChanges) ? dcr.conferenceChanges : []
  for (const change of changes.slice(0, DCR_CHANGES_LISTED)) {
    const kind = change?.kind === 'joined' || change?.kind === 'left' ? change.kind : 'joined or left'
    notices.push(
      `A remote participant ${kind} at ${clockTime(change?.atSeconds)}; DepoAudio's channel order after that point is a best guess.`,
    )
  }
  const total = Number.isInteger(dcr.conferenceChangeCount) ? dcr.conferenceChangeCount : changes.length
  const unlisted = total - Math.min(changes.length, DCR_CHANGES_LISTED)
  if (unlisted > 0) {
    notices.push(
      `${unlisted} more remote-participant ${unlisted === 1 ? 'change follows' : 'changes follow'}; the export record lists them.`,
    )
  }
  const dropped = Number.isInteger(dcr.trailingBytesDropped) ? dcr.trailingBytesDropped : 0
  if (dropped > 0) {
    notices.push(
      `The last ${dropped} ${dropped === 1 ? 'byte' : 'bytes'} of audio are too few for one sample of every channel and are left out; the export record says so.`,
    )
  }
  if (dcr.videoFrames > 0 || dcr.attachments > 0) {
    notices.push('This recording also holds video frames or attachments; only the audio is converted.')
  }
  return notices
}
