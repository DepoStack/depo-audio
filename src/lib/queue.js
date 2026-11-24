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
const boundaryMissing = (file, mark, files, side) =>
  `The ${file} queued file of this recording has no ${mark}-boundary mark, so FTR would accept ${files} files ${side} it; if there are any, add them too.`
export const TRM_START_BOUNDARY_MISSING = boundaryMissing('earliest', 'start', 'earlier', 'before')
export const TRM_END_BOUNDARY_MISSING = boundaryMissing('latest', 'end', 'later', 'after')

const TRM_PLAIN_CODECS = { mp2: 'MPEG audio layer 2', mp3: 'MP3', wma: 'Windows Media Audio' }

// Places where DepoAudio's split put AAC frames that add no new sound in
// every channel for rounds of frames missing mid-recording
// (`silenceFilledGaps`), so later audio keeps its time; the export record
// lists where (`source.trm.silenceFilled`). The filled stretch is not digital
// silence: the decoder fades the sound before it out across it.
export function trmSilenceFilledNotice(gaps) {
  const count = Number(gaps)
  if (!Number.isInteger(count) || count < 1) return ''
  return `${count} damaged ${count === 1 ? 'section was' : 'sections were'} filled to keep the timing, adding no new sound (the sound just before each fades out across it); see the export record.`
}

// A file whose frames mark only the first stream (`streamMarking`
// stream-0-only) is split by giving frames to the streams in turn, so a lost
// or repeated frame cannot be seen there. Said only when DepoAudio's own
// split is the route.
export const TRM_UNMARKED_SPLIT_NOTICE =
  "Only this recording's first stream marks its frames, so a missing or repeated frame would put every later frame in the wrong channel, unseen. See the export record."

export function trmRecordingNotice(info) {
  if (info?.trmRefusal) return String(info.trmRefusal)
  const trm = info?.trm
  if (!trm) return ''
  const parts = []
  if (trm.openEnded) parts.push(TRM_OPEN_ENDED_NOTICE)
  const plain = TRM_PLAIN_CODECS[trm.audioCodec]
  if (plain) parts.push(`This FTR recording holds ${plain} audio, which FFmpeg reads as ordinary audio.`)
  if (trm.signature) parts.push(TRM_SIGNED_NOTICE)
  const filled = trmSilenceFilledNotice(trm.silenceFilledGaps)
  if (filled) parts.push(filled)
  if (trm.streamMarking === 'stream-0-only' && trm.decodePath === 'deinterleaved') parts.push(TRM_UNMARKED_SPLIT_NOTICE)
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
    if (key) places.set(key, [...(places.get(key) || []), file.path])
  }
  const groups = []
  for (const paths of places.values()) {
    let last
    for (const path of paths.sort((x, y) => ftrRecordingDate(x).localeCompare(ftrRecordingDate(y)))) {
      const day = dateMs(ftrRecordingDate(path))
      // A new run when the date is more than a day after the last one.
      if (!(day - last <= DAY_MS)) groups.push([])
      groups[groups.length - 1].push(path)
      last = day
    }
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
    if (!routes.every(Boolean)) continue
    let first = 0
    routes.forEach((route, index) => {
      // A recording ends at an end boundary, or where the next file starts one.
      const next = routes[index + 1]
      if (next && !next.startBoundary && !route.endBoundary) return
      const parts = []
      if (!routes[first].startBoundary) parts.push(TRM_START_BOUNDARY_MISSING)
      if (!route.endBoundary) parts.push(TRM_END_BOUNDARY_MISSING)
      if (parts.length > 0) notices[paths[first]] = parts.join(' ')
      first = index + 1
    })
  }
  return notices
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
    const after = Number.isInteger(change?.channelsAfter)
      ? ` (${change.channelsAfter} ${change.channelsAfter === 1 ? 'channel' : 'channels'} recorded from then on)`
      : ''
    notices.push(`A remote participant ${kind} at ${clockTime(change?.atSeconds)}${after}.`)
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
  // Redactions, password-protected sections, checksums and the like, worded
  // by the reader (never from the recording's notes).
  if (Array.isArray(dcr.notices)) {
    for (const notice of dcr.notices) {
      if (typeof notice === 'string' && notice) notices.push(notice)
    }
  }
  return notices
}
