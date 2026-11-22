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

// Byte counts in the queue's notices: exact below 1 KB, else one decimal.
function formatUnreadBytes(bytes) {
  if (bytes < 1024) return `${bytes} ${bytes === 1 ? 'byte' : 'bytes'}`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`
}

// Why a recording session is not read, as the queue says it.
function omittedSessionSentence({ number, reason, last }) {
  const which = last ? `The last session (${number})` : `Session ${number}`
  if (reason === 'zero-length-header') return `${which} could not be read: its header says 0 bytes.`
  if (reason === 'no-complete-audio') return `${which} could not be read: it holds no complete audio yet.`
  if (reason === 'empty') return `${which} holds no audio: its header says 0 bytes.`
  return `${which} could not be read.`
}

// Plain notices for an SGMCA (Direct Audio) file, from probe_audio_streams_cmd:
// how many recording sessions it holds (they are joined in file order), which
// sessions and how many bytes are not read (a recording still growing, or one
// that stopped mid-session), and whether it looked still recording or cut
// short. The same facts go into the export record (source.sgmca.unreadBytes
// and omittedSegments). No answer means no notice.
export function recordingSessionNotice(info) {
  const notices = []
  const sessions = info?.recordingSessions
  if (Number.isInteger(sessions) && sessions > 1) {
    notices.push(
      `This file holds ${sessions} recording sessions; they are joined in order (gaps between them are not kept).`,
    )
  }
  const omitted = Array.isArray(info?.omittedSessions) ? info.omittedSessions : []
  for (const session of omitted) notices.push(omittedSessionSentence(session))
  const cutShort = info?.cutShortSession
  if (Number.isInteger(cutShort)) {
    notices.push(`The last session (${cutShort}) is shorter than its header says; only its complete pages are read.`)
  }
  const unread = info?.unreadBytes
  if (Number.isInteger(unread) && unread > 0) {
    const after =
      omitted.some(session => session.last) || Number.isInteger(cutShort) ? 'after it' : 'at the end of the file'
    notices.push(`${formatUnreadBytes(unread)} ${after} ${unread === 1 ? 'was' : 'were'} not read.`)
  }
  if (info?.stillRecording === true) {
    notices.push(
      'This file looks like it is still being recorded, or it was cut short; only its complete audio is converted.',
    )
  }
  return notices.join(' ')
}
