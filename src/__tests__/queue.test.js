import { describe, expect, it } from 'vitest'
import {
  audioTrackNotice,
  catJobPaths,
  dcrQueueNotices,
  mergeQueuedFiles,
  partitionQueuePaths,
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

describe('dcrQueueNotices', () => {
  const dcr = extra => ({
    convertedIndex: 0,
    streams: [{ channels: 3, sampleRate: 8000 }],
    durationSecs: 3600,
    dcr: {
      codec: 'PCM 16-bit',
      channelNames: ['Judge', 'Witness', 'Ch 3'],
      conferenceChanges: [],
      conferenceChangeCount: 0,
      videoFrames: 0,
      attachments: 0,
      problem: null,
      ...extra,
    },
  })

  it('says nothing for files the DCR reader did not read', () => {
    expect(dcrQueueNotices(null)).toEqual([])
    expect(dcrQueueNotices({ streams: [] })).toEqual([])
  })

  it('names the channels read from the recording', () => {
    expect(dcrQueueNotices(dcr())).toEqual([
      'Channels read from the recording: Judge, Witness, Ch 3. Split outputs and export records use these names instead of the channel labels below.',
    ])
  })

  it('lists each remote-participant change with the channel count after it, and counts the rest', () => {
    const change = (atSeconds, kind = 'joined', channelsAfter = 3) => ({
      atSeconds,
      kind,
      channelMask: 4,
      channelsAfter,
    })
    const notices = dcrQueueNotices(
      dcr({
        conferenceChanges: [change(754), change(3600, 'left', 1), change(3725.5, 'other', null), change(4000)],
        conferenceChangeCount: 5,
        videoFrames: 2,
      }),
    )
    expect(notices.slice(1, 4)).toEqual([
      'A remote participant joined at 00:12:34 (3 channels recorded from then on).',
      'A remote participant left at 01:00:00 (1 channel recorded from then on).',
      'A remote participant joined or left at 01:02:05.',
    ])
    expect(notices[4]).toBe('2 more remote-participant changes follow; the export record lists them.')
    expect(notices[5]).toBe('This recording also holds video frames or attachments; only the audio is converted.')
    expect(notices.join(' ')).not.toContain('guess')
  })

  it("shows the reader's redaction, protection and checksum notices after the others", () => {
    const notices = dcrQueueNotices(
      dcr({
        notices: [
          '2 redacted segments: 1 left out, 1 silenced, as marked in the recording.',
          'This recording has password-protected sections; they are silent in the output.',
          42,
          '',
        ],
      }),
    )
    expect(notices.slice(1)).toEqual([
      '2 redacted segments: 1 left out, 1 silenced, as marked in the recording.',
      'This recording has password-protected sections; they are silent in the output.',
    ])
  })

  it('says when bytes at the end of the audio are left out', () => {
    expect(dcrQueueNotices(dcr({ trailingBytesDropped: 2 }))[1]).toBe(
      'The last 2 bytes of audio are too few for one sample of every channel and are left out; the export record says so.',
    )
    expect(dcrQueueNotices(dcr({ trailingBytesDropped: 0 }))).toHaveLength(1)
  })

  it('shows only the reason when the recording cannot be converted', () => {
    const problem =
      "A redaction in this recording has no position, so DepoAudio can't tell which audio it covers. Export it to WAV in the software that recorded it (for example Liberty Court Recorder or BIS Digital), then add the WAV."
    expect(dcrQueueNotices(dcr({ problem }))).toEqual([problem])
  })
})
