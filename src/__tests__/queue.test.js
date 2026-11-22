import { describe, expect, it } from 'vitest'
import {
  audioTrackNotice,
  catJobPaths,
  dmNotice,
  mergeQueuedFiles,
  partitionQueuePaths,
  recordingSessionNotice,
  reexportConfirmCopy,
} from '../lib/queue'

describe('partitionQueuePaths', () => {
  it('separates TRS companions without discarding any dropped TRM chunks', () => {
    expect(
      partitionQueuePaths([
        '/case/session.TRS',
        '/case/chunk-2.trm',
        { path: '/case/chunk-1.trm' },
        '/case/chunk-2.trm',
      ]),
    ).toEqual({
      queuePaths: ['/case/chunk-2.trm', '/case/chunk-1.trm'],
      trsCompanions: ['/case/session.TRS'],
    })
  })
})

describe('mergeQueuedFiles', () => {
  const early = { path: '/case/CR24_20180621-1449_01d4096f0757ee50.trm', name: 'early.trm' }
  const middle = { path: '/case/CR24_20180621-1454_01d4096fbaa19b00.trm', name: 'middle.trm' }
  const late = { path: '/case/CR24_20180621-1459_01d409706d824cb0.trm', name: 'late.trm' }

  it('dedupes and chronologically sorts the combined FTR queue across additions', () => {
    const merged = mergeQueuedFiles([late], [middle, early, late])
    expect(merged.map(file => file.path)).toEqual([early.path, middle.path, late.path])
  })

  it('can atomically replace an existing queue', () => {
    const merged = mergeQueuedFiles([{ path: '/old.wav' }], [late, early], { replace: true })
    expect(merged.map(file => file.path)).toEqual([early.path, late.path])
  })
})

describe('catJobPaths', () => {
  it('returns every valid file path in a detected court-software job', () => {
    expect(catJobPaths({ files: [{ path: '/one.trm' }, { path: '/two.trm' }, {}] })).toEqual(['/one.trm', '/two.trm'])
  })
})

describe('reexportConfirmCopy', () => {
  it('counts the queue and the Library files with the right number', () => {
    const many = reexportConfirmCopy({ queued: 3, queuedAtOpen: 3, incoming: 2 })
    expect(many.title).toBe('Replace the 3 files in the queue?')
    expect(many.confirmLabel).toBe('Replace 3 files')
    expect(many.description).toBe(
      'The 3 files waiting in Convert are taken out of the queue. Nothing is deleted from disk. Add to queue keeps them and adds the 2 Library files after them.',
    )
    expect(many.destructive).toBe(true)

    const one = reexportConfirmCopy({ queued: 1, queuedAtOpen: 1, incoming: 1 })
    expect(one.title).toBe('Replace the 1 file in the queue?')
    expect(one.description).not.toMatch(/\bfiles\b/)
  })

  it('names files added while the dialog was open', () => {
    expect(reexportConfirmCopy({ queued: 4, queuedAtOpen: 2, incoming: 1 }).description).toContain(
      '2 files were added to the queue while this was open and are counted here.',
    )
    expect(reexportConfirmCopy({ queued: 1, queuedAtOpen: 2, incoming: 1 }).description).not.toContain('added to the')
  })
})

describe('audioTrackNotice', () => {
  const stream = (language, title, channels = 2) => ({ channels, sampleRate: 48000, language, title })

  it('names the converted track with its language and title', () => {
    expect(
      audioTrackNotice({ convertedIndex: 0, streams: [stream('eng', 'Floor', 1), stream('spa', 'Interpreter', 4)] }),
    ).toBe('This file has 2 audio tracks; converting track 1 (eng, "Floor"). The other track is not converted.')
  })

  it('leaves out tags the track does not have', () => {
    expect(audioTrackNotice({ convertedIndex: 0, streams: [stream(null, null), stream(null, null), stream()] })).toBe(
      'This file has 3 audio tracks; converting track 1. The other 2 tracks are not converted.',
    )
    expect(audioTrackNotice({ convertedIndex: 0, streams: [stream(null, 'Room 4'), stream('eng', null)] })).toContain(
      'converting track 1 ("Room 4")',
    )
  })

  it('says nothing for one track or no probe answer', () => {
    expect(audioTrackNotice({ convertedIndex: 0, streams: [stream('eng', 'Floor')] })).toBe('')
    expect(audioTrackNotice(null)).toBe('')
    expect(audioTrackNotice(undefined)).toBe('')
    expect(audioTrackNotice([])).toBe('')
  })
})

describe('recordingSessionNotice', () => {
  const sgmca = (recordingSessions, stillRecording = false) => ({
    convertedIndex: 0,
    streams: [{ channels: 4, sampleRate: 48000, language: null, title: null }],
    durationSecs: 10,
    recordingSessions,
    stillRecording,
  })

  it('says how many sessions are joined, and that gaps are not kept', () => {
    expect(recordingSessionNotice(sgmca(3))).toBe(
      'This file holds 3 recording sessions; they are joined in order (gaps between them are not kept).',
    )
  })

  it('says when the file looks still recording or cut short', () => {
    expect(recordingSessionNotice(sgmca(1, true))).toBe(
      'This file looks like it is still being recorded, or it was cut short; only its complete audio is converted.',
    )
    expect(recordingSessionNotice(sgmca(2, true))).toContain('2 recording sessions')
    expect(recordingSessionNotice(sgmca(2, true))).toContain('still being recorded')
  })

  it('names a last session left out and the bytes after it before converting', () => {
    const info = {
      ...sgmca(2, true),
      unreadBytes: 12_897_485,
      omittedSessions: [{ number: 3, reason: 'zero-length-header', last: true }],
    }
    expect(recordingSessionNotice(info)).toBe(
      'This file holds 2 recording sessions; they are joined in order (gaps between them are not kept). ' +
        'The last session (3) could not be read: its header says 0 bytes. 12.3 MB after it were not read. ' +
        'This file looks like it is still being recorded, or it was cut short; only its complete audio is converted.',
    )
  })

  it('names every other way a session or bytes are left out', () => {
    expect(
      recordingSessionNotice({
        ...sgmca(1, true),
        unreadBytes: 40,
        omittedSessions: [{ number: 2, reason: 'no-complete-audio', last: true }],
      }),
    ).toContain(
      'The last session (2) could not be read: it holds no complete audio yet. 40 bytes after it were not read.',
    )
    expect(
      recordingSessionNotice({
        ...sgmca(2),
        unreadBytes: 0,
        omittedSessions: [{ number: 2, reason: 'empty', last: false }],
      }),
    ).toContain('Session 2 holds no audio: its header says 0 bytes.')
    expect(recordingSessionNotice({ ...sgmca(1, true), unreadBytes: 1, cutShortSession: 1 })).toContain(
      'The last session (1) is shorter than its header says; only its complete pages are read. 1 byte after it was not read.',
    )
    expect(recordingSessionNotice({ ...sgmca(1, true), unreadBytes: 2048, omittedSessions: [] })).toContain(
      '2.0 KB at the end of the file were not read.',
    )
  })

  it('stays quiet for one complete session and for any other file', () => {
    expect(recordingSessionNotice(sgmca(1))).toBe('')
    expect(recordingSessionNotice({ convertedIndex: 0, streams: [] })).toBe('')
    expect(recordingSessionNotice(null)).toBe('')
    expect(recordingSessionNotice(undefined)).toBe('')
  })
})

describe('dmNotice', () => {
  it('says how many recorded-over blocks are skipped', () => {
    const dm = { blocksErased: 12, stampsMonotonic: true }
    expect(dmNotice({ streams: [{}], dm })).toBe('12 recorded-over blocks skipped.')
    expect(dmNotice({ dm: { blocksErased: 1, stampsMonotonic: true } })).toBe('1 recorded-over block skipped.')
  })
  it('says when stamps go backwards and audio stays in file order, without naming a cause', () => {
    const backwards = 'Time stamps go backwards in places; the file does not say why. Audio is kept in file order.'
    expect(dmNotice({ dm: { blocksErased: 0, stampsMonotonic: false } })).toBe(backwards)
    // The notes say a record-over appends later stamps, so it is not offered as a cause.
    expect(dmNotice({ dm: { blocksErased: 0, stampsMonotonic: false } })).not.toMatch(/record/i)
    expect(dmNotice({ dm: { blocksErased: 3, stampsMonotonic: false } })).toBe(
      `3 recorded-over blocks skipped. ${backwards}`,
    )
  })
  it('says when a file ends partway through a block or reading stopped at a short block', () => {
    expect(dmNotice({ dm: { blocksErased: 0, stampsMonotonic: true, truncatedTailBytes: 300 } })).toBe(
      'The file ends partway through a block, so it may not have finished recording or copying; the complete blocks are converted and the incomplete one is left out.',
    )
    expect(dmNotice({ dm: { stampsMonotonic: true, shortFinalBlock: true, bytesAfterShortBlock: 4220 } })).toBe(
      "A short block comes before the end of the file. The format notes say SoundSync's player stops there, and so does DepoAudio: the 4220 bytes after it are not converted.",
    )
    expect(dmNotice({ dm: { stampsMonotonic: true, bytesAfterShortBlock: 1 } })).toBe(
      "A short block comes before the end of the file. The format notes say SoundSync's player stops there, and so does DepoAudio: the byte after it is not converted.",
    )
    expect(dmNotice({ dm: { stampsMonotonic: true, shortFinalBlock: true, bytesAfterShortBlock: 0 } })).toBe(
      "The last block is shorter than the others. The format notes say SoundSync's player does not play it; its whole codec blocks are converted.",
    )
    expect(
      dmNotice({
        dm: { stampsMonotonic: true, shortFinalBlock: false, bytesAfterShortBlock: 0, truncatedTailBytes: 0 },
      }),
    ).toBe('')
  })
  it('is empty with nothing to report or no answer', () => {
    expect(dmNotice({ dm: { blocksErased: 0, stampsMonotonic: true } })).toBe('')
    expect(dmNotice({ streams: [{}] })).toBe('')
    expect(dmNotice(null)).toBe('')
    expect(dmNotice({ dm: { blocksErased: 'many' } })).toBe('')
  })
})
